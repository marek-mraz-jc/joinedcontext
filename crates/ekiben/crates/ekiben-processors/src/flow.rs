use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ekiben_bloblang::{Executor, Value};
use ekiben_core::{
    execute_all, execute_catch_all, execute_try_all, BackoffConfig, Batch, ComponentConfig,
    CoreError, Part, PartsView, ProcContext, Processor, Registry,
};
use serde_json::Value as JsonValue;

pub fn register(registry: &Registry) {
    registry.register_processor("noop", |_conf, _ctx| Ok(Arc::new(NoopProcessor)));

    registry.register_processor("try", |conf, ctx| {
        let children = parse_child_processor_list(&conf.config, ctx)?;
        Ok(Arc::new(TryProcessor { children }))
    });

    registry.register_processor("catch", |conf, ctx| {
        let children = parse_child_processor_list(&conf.config, ctx)?;
        Ok(Arc::new(CatchProcessor { children }))
    });

    registry.register_processor("for_each", |conf, ctx| {
        let children = parse_child_processor_list(&conf.config, ctx)?;
        Ok(Arc::new(ForEachProcessor { children }))
    });

    registry.register_processor("processors", |conf, ctx| {
        let children = parse_child_processor_list(&conf.config, ctx)?;
        Ok(Arc::new(ProcessorsGroup { children }))
    });

    registry.register_processor("while", |conf, ctx| {
        Ok(Arc::new(WhileProcessor::from_config(conf, ctx)?))
    });

    registry.register_processor("parallel", |conf, ctx| {
        Ok(Arc::new(ParallelProcessor::from_config(conf, ctx)?))
    });

    registry.register_processor("retry", |conf, ctx| {
        Ok(Arc::new(RetryProcessor::from_config(conf, ctx)?))
    });
}

fn parse_child_processor_list(
    val: &JsonValue,
    ctx: &ProcContext,
) -> Result<Vec<Arc<dyn Processor>>, CoreError> {
    let Some(arr) = val.as_array() else {
        return Ok(Vec::new());
    };
    let mut confs = Vec::with_capacity(arr.len());
    for item in arr {
        confs.push(ComponentConfig::from_json_value(item, "noop", None)?);
    }
    ctx.registry.build_processor_list(&confs, ctx)
}

pub struct NoopProcessor;

#[async_trait]
impl Processor for NoopProcessor {
    fn processor_type(&self) -> &'static str {
        "noop"
    }

    async fn process_batch(
        &self,
        _ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        Ok(vec![batch])
    }
}

/// Implements `try` (`internal/impl/pure/processor_try.go`):
/// splits batch into 1-part batches, skips any part that has failed a prior step, and flattens at the end.
pub struct TryProcessor {
    children: Vec<Arc<dyn Processor>>,
}

#[async_trait]
impl Processor for TryProcessor {
    fn processor_type(&self) -> &'static str {
        "try"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let individual: Vec<Batch> = batch.into_iter().map(|p| vec![p]).collect();
        let res_batches = execute_try_all(ctx, &self.children, individual).await?;
        let mut out = Vec::new();
        for b in res_batches {
            out.extend(b);
        }
        if out.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![out])
        }
    }

    async fn close(&self) -> Result<(), CoreError> {
        for c in &self.children {
            c.close().await?;
        }
        Ok(())
    }
}

/// Implements `catch` (`internal/impl/pure/processor_catch.go`):
/// runs child processors only on parts that entered with an error flag, and clears all error flags upon exit!
pub struct CatchProcessor {
    children: Vec<Arc<dyn Processor>>,
}

#[async_trait]
impl Processor for CatchProcessor {
    fn processor_type(&self) -> &'static str {
        "catch"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let individual: Vec<Batch> = batch.into_iter().map(|p| vec![p]).collect();
        let res_batches = execute_catch_all(ctx, &self.children, individual).await?;
        let mut out = Vec::new();
        for b in res_batches {
            for mut p in b {
                p.clear_error();
                out.push(p);
            }
        }
        if out.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![out])
        }
    }

    async fn close(&self) -> Result<(), CoreError> {
        for c in &self.children {
            c.close().await?;
        }
        Ok(())
    }
}

/// Implements `for_each` (`internal/impl/pure/processor_for_each.go`).
pub struct ForEachProcessor {
    children: Vec<Arc<dyn Processor>>,
}

#[async_trait]
impl Processor for ForEachProcessor {
    fn processor_type(&self) -> &'static str {
        "for_each"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let mut out = Vec::new();
        for p in batch {
            let res_batches = execute_all(ctx, &self.children, vec![vec![p]]).await?;
            for b in res_batches {
                out.extend(b);
            }
        }
        if out.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![out])
        }
    }

    async fn close(&self) -> Result<(), CoreError> {
        for c in &self.children {
            c.close().await?;
        }
        Ok(())
    }
}

/// Implements `processors` composite group (`internal/impl/pure/processor_processors.go`).
pub struct ProcessorsGroup {
    children: Vec<Arc<dyn Processor>>,
}

#[async_trait]
impl Processor for ProcessorsGroup {
    fn processor_type(&self) -> &'static str {
        "processors"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        execute_all(ctx, &self.children, vec![batch]).await
    }

    async fn close(&self) -> Result<(), CoreError> {
        for c in &self.children {
            c.close().await?;
        }
        Ok(())
    }
}

const HARD_MAX_WHILE_LOOPS: usize = 10_000;

/// Implements `while` (`internal/impl/pure/processor_while.go`):
/// checks `check.query_part(0, &PartsView(&msgs[0]))` (first part of first batch!).
pub struct WhileProcessor {
    at_least_once: bool,
    max_loops: usize,
    check: Executor,
    children: Vec<Arc<dyn Processor>>,
}

impl WhileProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("while processor config must be an object"))?;

        let at_least_once = obj
            .get("at_least_once")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let max_loops = obj
            .get("max_loops")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as usize;
        let check_str = obj
            .get("check")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| CoreError::config("while processor requires a non-empty check query"))?;

        let check = ctx
            .blobl_env
            .compile_mapping(check_str)
            .map_err(|e| CoreError::config(format!("failed to parse check query: {e}")))?;

        let children = match obj.get("processors") {
            Some(v) => parse_child_processor_list(v, ctx)?,
            None => Vec::new(),
        };

        Ok(Self {
            at_least_once,
            max_loops,
            check,
            children,
        })
    }

    fn check_batch(&self, batch: &Batch) -> bool {
        if batch.is_empty() {
            return false;
        }
        self.check.query_part(0, &PartsView(batch)).unwrap_or(false)
    }
}

#[async_trait]
impl Processor for WhileProcessor {
    fn processor_type(&self) -> &'static str {
        "while"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let mut msgs = vec![batch];
        let mut loops = 0usize;
        let mut cond = self.at_least_once || self.check_batch(&msgs[0]);

        while cond {
            if ctx.is_shutting_down() {
                return Err(CoreError::ComponentClosed);
            }
            if self.max_loops > 0 && loops >= self.max_loops {
                break;
            }
            if loops >= HARD_MAX_WHILE_LOOPS {
                return Err(CoreError::other(format!(
                    "while processor exceeded hard safety loop limit ({HARD_MAX_WHILE_LOOPS})"
                )));
            }
            msgs = execute_all(ctx, &self.children, msgs).await?;
            if msgs.is_empty() || msgs[0].is_empty() {
                return Ok(Vec::new());
            }
            cond = self.check_batch(&msgs[0]);
            loops += 1;
        }

        Ok(msgs)
    }

    async fn close(&self) -> Result<(), CoreError> {
        for c in &self.children {
            c.close().await?;
        }
        Ok(())
    }
}

/// Implements `parallel` (`internal/impl/pure/processor_parallel.go`).
pub struct ParallelProcessor {
    cap: usize,
    children: Vec<Arc<dyn Processor>>,
}

impl ParallelProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("parallel processor config must be an object"))?;
        let cap = obj.get("cap").and_then(JsonValue::as_u64).unwrap_or(0) as usize;
        let children = match obj.get("processors") {
            Some(v) => parse_child_processor_list(v, ctx)?,
            None => Vec::new(),
        };
        Ok(Self { cap, children })
    }
}

#[async_trait]
impl Processor for ParallelProcessor {
    fn processor_type(&self) -> &'static str {
        "parallel"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let max_conc = if self.cap == 0 || batch.len() < self.cap {
            batch.len()
        } else {
            self.cap
        };
        let sem = Arc::new(tokio::sync::Semaphore::new(max_conc.max(1)));
        let mut futs = Vec::with_capacity(batch.len());

        for part in batch {
            let permit_sem = sem.clone();
            let children = self.children.clone();
            let c_ctx = ctx.clone();
            futs.push(async move {
                let _permit = permit_sem.acquire().await;
                execute_all(&c_ctx, &children, vec![vec![part]]).await
            });
        }

        let results = futures::future::join_all(futs).await;
        let mut out = Vec::new();
        for res in results {
            for b in res? {
                out.extend(b);
            }
        }
        if out.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![out])
        }
    }

    async fn close(&self) -> Result<(), CoreError> {
        for c in &self.children {
            c.close().await?;
        }
        Ok(())
    }
}

/// Implements `retry` (`internal/impl/pure/processor_retry.go`).
pub struct RetryProcessor {
    backoff: BackoffConfig,
    parallel: bool,
    max_retries: usize,
    children: Vec<Arc<dyn Processor>>,
}

impl RetryProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("retry processor config must be an object"))?;

        let backoff = match obj.get("backoff") {
            Some(v) => BackoffConfig::from_json_value(v, Duration::from_secs(30))?,
            None => BackoffConfig {
                initial_interval: Duration::from_millis(500),
                max_interval: Duration::from_secs(3),
                max_elapsed_time: Duration::from_secs(30),
            },
        };
        let parallel = obj
            .get("parallel")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let max_retries = obj
            .get("max_retries")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as usize;
        let children = match obj.get("processors") {
            Some(v) => parse_child_processor_list(v, ctx)?,
            None => Vec::new(),
        };
        if children.is_empty() {
            return Err(CoreError::config(
                "retry processor requires at least one child processor",
            ));
        }

        Ok(Self {
            backoff,
            parallel,
            max_retries,
            children,
        })
    }

    async fn dispatch_message(
        &self,
        ctx: &ProcContext,
        mut part: Part,
    ) -> Result<Vec<Batch>, CoreError> {
        part.clear_error();
        let mut retries = 0usize;
        let mut backoff_duration = Duration::ZERO;

        let res_batches = loop {
            let batches = execute_all(ctx, &self.children, vec![vec![part.shallow_copy()]]).await?;
            let has_failed = batches.iter().any(|b| b.iter().any(Part::is_errored));
            if !has_failed {
                break batches;
            }
            retries += 1;
            if self.max_retries > 0 && retries >= self.max_retries {
                break batches;
            }
            let Some(next_sleep) = self
                .backoff
                .next_backoff((retries - 1) as u32, backoff_duration)
            else {
                break batches;
            };
            backoff_duration += next_sleep;
            tokio::time::sleep(next_sleep).await;
            if ctx.is_shutting_down() {
                return Err(CoreError::ComponentClosed);
            }
        };

        let mut tagged = res_batches;
        for b in &mut tagged {
            for m in b {
                m.meta_set("retry_count", Value::Int(retries as i64));
                m.meta_set(
                    "backoff_duration",
                    Value::Int(backoff_duration.as_nanos() as i64),
                );
            }
        }
        Ok(tagged)
    }
}

#[async_trait]
impl Processor for RetryProcessor {
    fn processor_type(&self) -> &'static str {
        "retry"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let mut out = Vec::new();
        if self.parallel {
            let mut futs = Vec::with_capacity(batch.len());
            for p in batch {
                futs.push(self.dispatch_message(ctx, p));
            }
            for res in futures::future::join_all(futs).await {
                for b in res? {
                    out.extend(b);
                }
            }
        } else {
            for p in batch {
                for b in self.dispatch_message(ctx, p).await? {
                    out.extend(b);
                }
            }
        }
        if out.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![out])
        }
    }

    async fn close(&self) -> Result<(), CoreError> {
        for c in &self.children {
            c.close().await?;
        }
        Ok(())
    }
}
