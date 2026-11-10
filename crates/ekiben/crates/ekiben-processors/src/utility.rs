use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_bloblang::{Executor, FieldExpression};
use ekiben_core::config::parse_duration_str;
use ekiben_core::{
    Batch, ComponentConfig, CoreError, Part, PartMetaAdapter, PartsView, ProcContext, Processor,
    Registry,
};
use serde_json::Value as JsonValue;

pub fn register(registry: &Registry) {
    registry.register_processor("sleep", |conf, ctx| {
        Ok(Arc::new(SleepProcessor::from_config(conf, ctx)?))
    });

    registry.register_processor("log", |conf, ctx| {
        Ok(Arc::new(LogProcessor::from_config(conf, ctx)?))
    });

    registry.register_processor("resource", |conf, ctx| {
        let name = conf
            .config
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::config("resource processor requires a resource name string"))?
            .to_string();
        // Bento refuses to start a stream naming an undefined resource. Resources are registered before the
        // pipeline is built; one resource may still name another declared after it in the same list.
        if !ctx.path.starts_with("processor_resources")
            && !ctx.resources.has_processor(ctx.org_id(), &name)
        {
            return Err(CoreError::config(format!(
                "processor resource '{name}' was not found"
            )));
        }
        Ok(Arc::new(ResourceProcessor { name }))
    });

    registry.register_processor("rate_limit", |conf, _ctx| {
        let resource = conf
            .config
            .get("resource")
            .and_then(JsonValue::as_str)
            .or_else(|| conf.config.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::config("rate_limit processor requires a resource field"))?
            .to_string();
        Ok(Arc::new(RateLimitProcessor { resource }))
    });

    registry.register_processor("cache", |conf, ctx| {
        Ok(Arc::new(CacheProcessor::from_config(conf, ctx)?))
    });

    registry.register_processor("dedupe", |conf, ctx| {
        Ok(Arc::new(DedupeProcessor::from_config(conf, ctx)?))
    });

    registry.register_processor("bounds_check", |conf, _ctx| {
        Ok(Arc::new(BoundsCheckProcessor::from_config(conf)))
    });

    registry.register_processor("group_by_value", |conf, ctx| {
        Ok(Arc::new(GroupByValueProcessor::from_config(conf, ctx)?))
    });

    registry.register_processor("insert_part", |conf, ctx| {
        Ok(Arc::new(InsertPartProcessor::from_config(conf, ctx)?))
    });

    registry.register_processor("select_parts", |conf, _ctx| {
        Ok(Arc::new(SelectPartsProcessor::from_config(conf)))
    });

    registry.register_processor("metric", |conf, ctx| {
        Ok(Arc::new(MetricProcessor::from_config(conf, ctx)?))
    });

    registry.register_processor("sync_response", |_conf, _ctx| {
        Ok(Arc::new(SyncResponseProcessor))
    });
}

/// Implements `sleep` (`internal/impl/pure/processor_sleep.go`).
pub struct SleepProcessor {
    duration_expr: FieldExpression,
}

impl SleepProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let dur_str = conf
            .config
            .get("duration")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("sleep processor requires a duration field"))?;
        let duration_expr = ctx.blobl_env.compile_field(dur_str)?;
        Ok(Self { duration_expr })
    }
}

#[async_trait]
impl Processor for SleepProcessor {
    fn processor_type(&self) -> &'static str {
        "sleep"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let view = PartsView(&batch);
        for i in 0..batch.len() {
            let Ok(period_str) = self.duration_expr.resolve_string(i, &view) else {
                continue;
            };
            // Negative or zero durations do not sleep at all (as documented in interpolation.md!)
            let Ok(nanos) = ekiben_bloblang::time_format::parse_go_duration_nanos(&period_str)
            else {
                continue;
            };
            if nanos <= 0 {
                continue;
            }
            let dur = Duration::from_nanos(nanos as u64);
            let mut rx = ctx.shutdown_rx.clone();
            tokio::select! {
                _ = tokio::time::sleep(dur) => {}
                _ = rx.changed() => {
                    if *rx.borrow() {
                        return Err(CoreError::ComponentClosed);
                    }
                }
            }
        }
        Ok(vec![batch])
    }
}

/// Implements `log` (`internal/impl/pure/processor_log.go`).
pub struct LogProcessor {
    level: String,
    message: FieldExpression,
    fields_mapping: Option<Executor>,
}

impl LogProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let level = conf
            .config
            .get("level")
            .and_then(JsonValue::as_str)
            .unwrap_or("INFO")
            .to_ascii_uppercase();
        let msg_str = conf
            .config
            .get("message")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let message = ctx.blobl_env.compile_field(msg_str)?;
        let fields_mapping = match conf
            .config
            .get("fields_mapping")
            .and_then(JsonValue::as_str)
        {
            Some(s) if !s.trim().is_empty() => Some(ctx.blobl_env.compile_mapping(s)?),
            _ => None,
        };
        Ok(Self {
            level,
            message,
            fields_mapping,
        })
    }
}

#[async_trait]
impl Processor for LogProcessor {
    fn processor_type(&self) -> &'static str {
        "log"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let view = PartsView(&batch);
        for (i, part) in batch.iter().enumerate() {
            let msg_text = self
                .message
                .resolve_string(i, &view)
                .unwrap_or_else(|e| format!("interpolation error: {e}"));
            let extra_fields = if let Some(ref fm) = self.fields_mapping {
                let mut dummy_part = part.shallow_copy();
                let mut adapter = PartMetaAdapter(&mut dummy_part);
                fm.map_part(i, &view, &mut adapter, None)
                    .ok()
                    .flatten()
                    .map(|v| v.to_string_coerced())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            match self.level.as_str() {
                "ERROR" | "FATAL" => tracing::error!(
                    org = %ctx.org_id(),
                    stream = %ctx.stream_id,
                    fields = %extra_fields,
                    "{}",
                    msg_text
                ),
                "WARN" => tracing::warn!(
                    org = %ctx.org_id(),
                    stream = %ctx.stream_id,
                    fields = %extra_fields,
                    "{}",
                    msg_text
                ),
                "DEBUG" => tracing::debug!(
                    org = %ctx.org_id(),
                    stream = %ctx.stream_id,
                    fields = %extra_fields,
                    "{}",
                    msg_text
                ),
                "TRACE" => tracing::trace!(
                    org = %ctx.org_id(),
                    stream = %ctx.stream_id,
                    fields = %extra_fields,
                    "{}",
                    msg_text
                ),
                _ => tracing::info!(
                    org = %ctx.org_id(),
                    stream = %ctx.stream_id,
                    fields = %extra_fields,
                    "{}",
                    msg_text
                ),
            }
        }
        Ok(vec![batch])
    }
}

/// Implements `resource` processor (`internal/impl/pure/processor_resource.go`).
/// Looks up the processor resource strictly within `ctx.org_id()`.
pub struct ResourceProcessor {
    name: String,
}

#[async_trait]
impl Processor for ResourceProcessor {
    fn processor_type(&self) -> &'static str {
        "resource"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let target = ctx.resources.get_processor(ctx.org_id(), &self.name)?;
        target.process_batch(ctx, batch).await
    }
}

/// Implements `rate_limit` processor (`internal/impl/pure/processor_rate_limit.go`).
/// Looks up the rate_limit resource strictly within `ctx.org_id()`.
pub struct RateLimitProcessor {
    resource: String,
}

#[async_trait]
impl Processor for RateLimitProcessor {
    fn processor_type(&self) -> &'static str {
        "rate_limit"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let rl = ctx.resources.get_rate_limit(ctx.org_id(), &self.resource)?;
        for part in &batch {
            loop {
                let wait = rl.access(std::slice::from_ref(part)).await?;
                if wait.is_zero() {
                    break;
                }
                let mut rx = ctx.shutdown_rx.clone();
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = rx.changed() => {
                        if *rx.borrow() {
                            return Err(CoreError::ComponentClosed);
                        }
                    }
                }
            }
        }
        Ok(vec![batch])
    }
}

/// Implements `cache` processor (`internal/impl/pure/processor_cache.go`).
/// Looks up the cache resource strictly within `ctx.org_id()`.
pub struct CacheProcessor {
    resource: String,
    operator: String,
    key: FieldExpression,
    value: Option<FieldExpression>,
    ttl: Option<FieldExpression>,
}

impl CacheProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("cache processor config must be an object"))?;
        let resource = obj
            .get("resource")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("cache processor requires a resource field"))?
            .to_string();
        // Bento probes the resource when it builds the step and refuses the stream without it.
        ctx.resources.get_cache(ctx.org_id(), &resource)?;
        let operator = obj
            .get("operator")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("cache processor requires an operator field"))?
            .to_string();
        let key_str = obj
            .get("key")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("cache processor requires a key field"))?;
        let key = ctx.blobl_env.compile_field(key_str)?;
        let value = match obj.get("value").and_then(JsonValue::as_str) {
            Some(s) => Some(ctx.blobl_env.compile_field(s)?),
            None => None,
        };
        let ttl = match obj.get("ttl").and_then(JsonValue::as_str) {
            Some(s) if !s.is_empty() => Some(ctx.blobl_env.compile_field(s)?),
            _ => None,
        };
        Ok(Self {
            resource,
            operator,
            key,
            value,
            ttl,
        })
    }
}

#[async_trait]
impl Processor for CacheProcessor {
    fn processor_type(&self) -> &'static str {
        "cache"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let cache = ctx.resources.get_cache(ctx.org_id(), &self.resource)?;

        for i in 0..batch.len() {
            let key_res = {
                let view = PartsView(&batch);
                self.key.resolve_string(i, &view)
            };
            let key_str = match key_res {
                Ok(k) => k,
                Err(e) => {
                    ctx.on_error(&mut batch[i], e.to_string());
                    continue;
                }
            };
            let ttl_dur = if let Some(ref ttl_expr) = self.ttl {
                let view = PartsView(&batch);
                ttl_expr
                    .resolve_string(i, &view)
                    .ok()
                    .and_then(|s| parse_duration_str(&s).ok())
            } else {
                None
            };

            match self.operator.as_str() {
                "get" => match cache.get(&key_str).await {
                    Ok(bytes) => {
                        batch[i].set_bytes(bytes);
                    }
                    Err(e) => {
                        ctx.on_error(&mut batch[i], cache_op_error(&key_str, &e));
                    }
                },
                "set" => {
                    let val_bytes = if let Some(ref val_expr) = self.value {
                        let view = PartsView(&batch);
                        match val_expr.resolve_bytes(i, &view) {
                            Ok(b) => Bytes::from(b),
                            Err(e) => {
                                ctx.on_error(&mut batch[i], e.to_string());
                                continue;
                            }
                        }
                    } else {
                        batch[i].as_bytes()
                    };
                    if let Err(e) = cache.set(&key_str, val_bytes, ttl_dur).await {
                        ctx.on_error(&mut batch[i], cache_op_error(&key_str, &e));
                    }
                }
                "add" => {
                    let val_bytes = if let Some(ref val_expr) = self.value {
                        let view = PartsView(&batch);
                        match val_expr.resolve_bytes(i, &view) {
                            Ok(b) => Bytes::from(b),
                            Err(e) => {
                                ctx.on_error(&mut batch[i], e.to_string());
                                continue;
                            }
                        }
                    } else {
                        batch[i].as_bytes()
                    };
                    if let Err(e) = cache.add(&key_str, val_bytes, ttl_dur).await {
                        ctx.on_error(&mut batch[i], cache_op_error(&key_str, &e));
                    }
                }
                "delete" => {
                    if let Err(e) = cache.delete(&key_str).await {
                        ctx.on_error(&mut batch[i], cache_op_error(&key_str, &e));
                    }
                }
                "exists" => match cache.exists(&key_str).await {
                    Ok(exists) => {
                        batch[i].set_structured(ekiben_bloblang::Value::Bool(exists));
                    }
                    Err(e) => {
                        ctx.on_error(&mut batch[i], cache_op_error(&key_str, &e));
                    }
                },
                other => {
                    ctx.on_error(&mut batch[i], format!("unknown cache operator '{other}'"));
                }
            }
        }

        Ok(vec![batch])
    }
}

/// The error text Bento's `cache` processor flags a message with (internal/impl/pure/processor_cache.go).
fn cache_op_error(key: &str, e: &CoreError) -> String {
    match e {
        CoreError::KeyAlreadyExists(_) => format!("key already exists: {key}"),
        CoreError::KeyNotFound(_) => format!("operator failed for key '{key}': key does not exist"),
        other => format!("operator failed for key '{key}': {other}"),
    }
}

/// Implements `dedupe` processor using an org-scoped cache resource.
pub struct DedupeProcessor {
    cache: String,
    key: FieldExpression,
    drop_on_err: bool,
    /// `strategy: FIFO` keeps the first message per key, `LIFO` the last.
    fifo: bool,
}

impl DedupeProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("dedupe config must be an object"))?;
        let cache = obj
            .get("cache")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("dedupe requires a cache field"))?
            .to_string();
        ctx.resources.get_cache(ctx.org_id(), &cache)?;
        let key_str = obj
            .get("key")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("dedupe requires a key field"))?;
        let key = ctx.blobl_env.compile_field(key_str)?;
        let drop_on_err = obj
            .get("drop_on_err")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true);
        let fifo = match obj.get("strategy").and_then(JsonValue::as_str) {
            None | Some("FIFO") => true,
            Some("LIFO") => false,
            Some(other) => {
                return Err(CoreError::config(format!(
                    "dedupe strategy '{other}' is not one of FIFO, LIFO"
                )))
            }
        };
        Ok(Self {
            cache,
            key,
            drop_on_err,
            fifo,
        })
    }
}

#[async_trait]
impl Processor for DedupeProcessor {
    fn processor_type(&self) -> &'static str {
        "dedupe"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let cache = ctx.resources.get_cache(ctx.org_id(), &self.cache)?;
        let view = PartsView(&batch);
        let mut out = Vec::with_capacity(batch.len());

        // LIFO walks the batch from its end so the last message per key wins, then restores order.
        let order: Vec<usize> = if self.fifo {
            (0..batch.len()).collect()
        } else {
            (0..batch.len()).rev().collect()
        };
        for i in order {
            let part = &batch[i];
            let Ok(k) = self.key.resolve_string(i, &view) else {
                if !self.drop_on_err {
                    out.push(part.clone());
                }
                continue;
            };
            match cache.add(&k, Bytes::from_static(b"1"), None).await {
                Ok(()) => out.push(part.clone()),
                Err(CoreError::KeyAlreadyExists(_)) => {
                    // Duplicate: drop part
                }
                Err(_) => {
                    if !self.drop_on_err {
                        out.push(part.clone());
                    }
                }
            }
        }

        if !self.fifo {
            out.reverse();
        }
        if out.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![out])
        }
    }
}

pub struct BoundsCheckProcessor {
    max_part_size: usize,
    min_part_size: usize,
    max_parts: usize,
    min_parts: usize,
}

impl BoundsCheckProcessor {
    pub fn from_config(conf: &ComponentConfig) -> Self {
        let max_part_size = conf
            .config
            .get("max_part_size")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1_073_741_824) as usize;
        let min_part_size = conf
            .config
            .get("min_part_size")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1) as usize;
        let max_parts = conf
            .config
            .get("max_parts")
            .and_then(JsonValue::as_u64)
            .unwrap_or(100) as usize;
        let min_parts = conf
            .config
            .get("min_parts")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1) as usize;
        Self {
            max_part_size,
            min_part_size,
            max_parts,
            min_parts,
        }
    }
}

#[async_trait]
impl Processor for BoundsCheckProcessor {
    fn processor_type(&self) -> &'static str {
        "bounds_check"
    }

    async fn process_batch(
        &self,
        _ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        if batch.len() < self.min_parts || batch.len() > self.max_parts {
            return Ok(Vec::new());
        }
        // Bento drops the whole batch when any part is outside the size bounds.
        let fits = batch.iter().all(|p| {
            let len = p.as_bytes_ref().len();
            len >= self.min_part_size && len <= self.max_part_size
        });
        Ok(if fits { vec![batch] } else { Vec::new() })
    }
}

/// Implements `group_by_value` (used by `joinedcontext-portal` `render.rs` `routes()`!).
pub struct GroupByValueProcessor {
    value: FieldExpression,
}

impl GroupByValueProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let val_str = conf
            .config
            .get("value")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("group_by_value requires a value field"))?;
        let value = ctx.blobl_env.compile_field(val_str)?;
        Ok(Self { value })
    }
}

#[async_trait]
impl Processor for GroupByValueProcessor {
    fn processor_type(&self) -> &'static str {
        "group_by_value"
    }

    async fn process_batch(
        &self,
        _ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let view = PartsView(&batch);
        let mut order: Vec<String> = Vec::new();
        let mut groups: BTreeMap<String, Batch> = BTreeMap::new();

        for (i, part) in batch.iter().enumerate() {
            let k = self.value.resolve_string(i, &view).unwrap_or_default();
            if !groups.contains_key(&k) {
                order.push(k.clone());
                groups.insert(k.clone(), Vec::new());
            }
            groups.get_mut(&k).unwrap().push(part.clone());
        }

        let out: Vec<Batch> = order
            .into_iter()
            .filter_map(|k| groups.remove(&k))
            .collect();
        Ok(out)
    }
}

pub struct InsertPartProcessor {
    index: isize,
    content: FieldExpression,
}

impl InsertPartProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let index = conf
            .config
            .get("index")
            .and_then(JsonValue::as_i64)
            .unwrap_or(-1) as isize;
        let content_str = conf
            .config
            .get("content")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let content = ctx.blobl_env.compile_field(content_str)?;
        Ok(Self { index, content })
    }
}

#[async_trait]
impl Processor for InsertPartProcessor {
    fn processor_type(&self) -> &'static str {
        "insert_part"
    }

    async fn process_batch(
        &self,
        _ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let bytes = self
            .content
            .resolve_bytes(0, &PartsView(&batch))
            .unwrap_or_default();
        let new_part = Part::new(Bytes::from(bytes));
        let len = batch.len() as isize;
        let target_idx = if self.index < 0 {
            (len + self.index + 1).max(0) as usize
        } else {
            (self.index as usize).min(batch.len())
        };
        batch.insert(target_idx, new_part);
        Ok(vec![batch])
    }
}

pub struct SelectPartsProcessor {
    parts: Vec<isize>,
}

impl SelectPartsProcessor {
    pub fn from_config(conf: &ComponentConfig) -> Self {
        let parts = conf
            .config
            .get("parts")
            .and_then(JsonValue::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(JsonValue::as_i64)
                    .map(|i| i as isize)
                    .collect()
            })
            .unwrap_or_else(|| vec![0]);
        Self { parts }
    }
}

#[async_trait]
impl Processor for SelectPartsProcessor {
    fn processor_type(&self) -> &'static str {
        "select_parts"
    }

    async fn process_batch(
        &self,
        _ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let len = batch.len() as isize;
        let mut out = Vec::new();
        for &idx in &self.parts {
            let norm = if idx < 0 { len + idx } else { idx };
            if norm >= 0 && norm < len {
                out.push(batch[norm as usize].clone());
            }
        }
        if out.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![out])
        }
    }
}

pub struct MetricProcessor {
    name: String,
    value: Option<FieldExpression>,
}

impl MetricProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let name = conf
            .config
            .get("name")
            .and_then(JsonValue::as_str)
            .unwrap_or("custom_metric")
            .to_string();
        let value = match conf.config.get("value").and_then(JsonValue::as_str) {
            Some(s) if !s.is_empty() => Some(ctx.blobl_env.compile_field(s)?),
            _ => None,
        };
        Ok(Self { name, value })
    }
}

#[async_trait]
impl Processor for MetricProcessor {
    fn processor_type(&self) -> &'static str {
        "metric"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let view = PartsView(&batch);
        for i in 0..batch.len() {
            let delta = match &self.value {
                Some(v) => v
                    .resolve_string(i, &view)
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(1),
                None => 1,
            };
            ctx.metrics.incr_counter(&self.name, delta);
        }
        Ok(vec![batch])
    }
}

pub struct SyncResponseProcessor;

#[async_trait]
impl Processor for SyncResponseProcessor {
    fn processor_type(&self) -> &'static str {
        "sync_response"
    }

    async fn process_batch(
        &self,
        _ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        if let Some(first) = batch.first() {
            if let Some(store) = first.sync_response_store() {
                store.add(&batch);
            }
        }
        Ok(vec![batch])
    }
}
