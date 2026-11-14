//! `cached` processor (`internal/impl/pure/processor_cached.go`): runs `processors` once per `key`
//! and stores their result in the `cache` resource; a later message with the same key gets the
//! stored result instead. The stored bytes keep Bento's versioned layout (format 1, big endian).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_bloblang::{Executor, FieldExpression};
use ekiben_core::{
    execute_all, Batch, ComponentConfig, CoreError, Part, PartsView, ProcContext, Processor,
};
use serde_json::Value as JsonValue;

pub struct CachedProcessor {
    cache: String,
    key: FieldExpression,
    ttl: Option<FieldExpression>,
    processors: Vec<Arc<dyn Processor>>,
    skip_on: Option<Executor>,
}

impl CachedProcessor {
    /// Only a cache resource of the pipeline's own organization is accepted, as Bento's `HasCache`.
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("cached: expected an object"))?;
        let field = |name: &str| {
            obj.get(name)
                .and_then(JsonValue::as_str)
                .ok_or_else(|| CoreError::config(format!("field {name} is required")))
        };
        let cache = field("cache")?.to_string();
        if !ctx.resources.has_cache(ctx.org_id(), &cache) {
            return Err(CoreError::config(format!("cache named {cache} not found")));
        }
        let interp = |s: &str| {
            ctx.blobl_env
                .compile_field(s)
                .map_err(|e| CoreError::config(e.to_string()))
        };
        let key = interp(field("key")?)?;
        let ttl = obj.contains_key("ttl").then(|| field("ttl")).transpose()?;
        let ttl = ttl.map(interp).transpose()?;
        let skip_on = match obj.get("skip_on") {
            None => None,
            Some(_) => Some(
                ctx.blobl_env
                    .compile_mapping(field("skip_on")?)
                    .map_err(|e| CoreError::config(e.to_string()))?,
            ),
        };
        let list = obj
            .get("processors")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| CoreError::config("field processors is required"))?;
        let confs = list
            .iter()
            .map(|p| ComponentConfig::from_json_value(p, "noop", None))
            .collect::<Result<Vec<_>, _>>()?;
        let child = ctx.into_child("cached", ctx.label.clone(), "processors");
        let processors = ctx.registry.build_processor_list(&confs, &child)?;
        Ok(Self {
            cache,
            key,
            ttl,
            processors,
            skip_on,
        })
    }

    /// Bento's `Process` for one message: the cached or freshly processed result, or an error that
    /// flags the message.
    async fn process_one(
        &self,
        ctx: &ProcContext,
        batch: &[Part],
        i: usize,
    ) -> Result<Batch, String> {
        let view = PartsView(batch);
        let key = self
            .key
            .resolve_string(i, &view)
            .map_err(|e| format!("failed to interpolate key expression: {e}"))?;
        let ttl = match &self.ttl {
            None => None,
            Some(expr) => {
                let s = expr
                    .resolve_string(i, &view)
                    .map_err(|e| format!("failed to interpolate ttl expression: {e}"))?;
                let nanos =
                    ekiben_bloblang::time_format::parse_go_duration_nanos(&s).map_err(|e| {
                        format!(
                            "failed to parse ttl expression: {}",
                            go_duration_error(&s, &e)
                        )
                    })?;
                Some(Duration::from_nanos(u64::try_from(nanos).unwrap_or(0)))
            }
        };
        let cache = ctx
            .resources
            .get_cache(ctx.org_id(), &self.cache)
            .map_err(|e| e.to_string())?;
        match cache.get(&key).await {
            Ok(stored) => {
                return result_to_batch(&batch[i], &stored).map_err(|e| {
                    format!("failed to parse cached result, this indicates the data was not set by this processor: {e}")
                })
            }
            Err(CoreError::KeyNotFound(_)) => {}
            Err(e) => return Err(e.to_string()),
        }
        let results = execute_all(ctx, &self.processors, vec![vec![batch[i].clone()]])
            .await
            .map_err(|e| e.to_string())?;
        let mut should_cache = true;
        let mut collapsed = Vec::new();
        for b in results {
            let skip = self.should_skip(&b).unwrap_or_else(|e| {
                tracing::error!(
                    "skip_on check failed: {e}, caching will be skipped as a precaution"
                );
                true
            });
            should_cache &= !skip;
            collapsed.extend(b);
        }
        if should_cache {
            if let Err(e) = cache.set(&key, serialise_batch(&collapsed), ttl).await {
                tracing::error!("failed to write result to cache: {e}");
            }
        }
        Ok(collapsed)
    }

    fn should_skip(&self, batch: &[Part]) -> Result<bool, String> {
        let Some(pred) = &self.skip_on else {
            return Ok(false);
        };
        if batch.is_empty() {
            return Ok(false);
        }
        pred.query_part(0, &PartsView(batch))
            .map_err(|e| format!("failed to execute skip_on mapping: {e}"))
    }
}

#[async_trait]
impl Processor for CachedProcessor {
    fn processor_type(&self) -> &'static str {
        "cached"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let mut out = Vec::with_capacity(batch.len());
        for i in 0..batch.len() {
            match self.process_one(ctx, &batch, i).await {
                Ok(parts) => out.extend(parts),
                Err(e) => {
                    let mut part = batch[i].clone();
                    ctx.on_error(&mut part, e);
                    out.push(part);
                }
            }
        }
        Ok(if out.is_empty() {
            Vec::new()
        } else {
            vec![out]
        })
    }

    async fn close(&self) -> Result<(), CoreError> {
        for p in &self.processors {
            p.close().await?;
        }
        Ok(())
    }
}

/// Go's `time.ParseDuration` error text for `input`.
pub(crate) fn go_duration_error(input: &str, err: &str) -> String {
    if err.starts_with("missing unit") {
        format!("time: missing unit in duration {input:?}")
    } else if err.starts_with("unknown unit") {
        format!("time: {err}")
    } else {
        format!("time: invalid duration {input:?}")
    }
}

/// `cachedProcSerialiseBatch`: version 1, the message count, then each message's length and bytes.
fn serialise_batch(batch: &[Part]) -> Bytes {
    let mut buf = Vec::new();
    buf.extend_from_slice(&1u32.to_be_bytes());
    buf.extend_from_slice(&len_u32(batch.len()).to_be_bytes());
    for part in batch {
        let body = part.as_bytes_ref();
        buf.extend_from_slice(&len_u32(body.len()).to_be_bytes());
        buf.extend_from_slice(&body);
    }
    Bytes::from(buf)
}

fn len_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn take_u32(data: &[u8]) -> Result<(u32, &[u8]), String> {
    match data.split_first_chunk::<4>() {
        Some((n, rest)) => Ok((u32::from_be_bytes(*n), rest)),
        None => Err("message is too small to extract number".to_string()),
    }
}

/// `cachedProcResultToBatch`: each stored message is a copy of `msg` (its metadata) with the stored bytes.
fn result_to_batch(msg: &Part, stored: &[u8]) -> Result<Batch, String> {
    let (version, data) = take_u32(stored)
        .map_err(|e| format!("failed to extract serialisation format version: {e}"))?;
    if version != 1 {
        return Err(format!("invalid format version: {version}"));
    }
    let (count, mut data) =
        take_u32(data).map_err(|e| format!("failed to extract batch size: {e}"))?;
    let mut out = Vec::new();
    for i in 0..count {
        let (size, rest) =
            take_u32(data).map_err(|e| format!("failed to extract message {i} size: {e}"))?;
        let size = size as usize;
        if rest.len() < size {
            return Err(format!(
                "failed to extract message {i} size: input data ended unexpectedly"
            ));
        }
        let mut copy = msg.clone();
        copy.set_bytes(Bytes::copy_from_slice(&rest[..size]));
        out.push(copy);
        data = &rest[size..];
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_result_reads_back_with_the_message_metadata() {
        let mut msg = Part::from_str("in");
        msg.meta_set_str("kept", "yes");
        let stored = serialise_batch(&[Part::from_str("a"), Part::from_str("bc")]);
        assert_eq!(&stored[..], b"\0\0\0\x01\0\0\0\x02\0\0\0\x01a\0\0\0\x02bc");
        let back = result_to_batch(&msg, &stored).expect("version 1");
        assert_eq!(back.len(), 2);
        assert_eq!(&back[1].as_bytes_ref()[..], b"bc");
        assert_eq!(back[1].meta_get_str("kept"), "yes");
    }

    #[test]
    fn foreign_bytes_are_refused_with_bento_texts() {
        let msg = Part::from_str("in");
        assert_eq!(
            result_to_batch(&msg, b"ab").expect_err("short"),
            "failed to extract serialisation format version: message is too small to extract number"
        );
        assert_eq!(
            result_to_batch(&msg, b"\0\0\0\x02").expect_err("v2"),
            "invalid format version: 2"
        );
        assert_eq!(
            result_to_batch(&msg, b"\0\0\0\x01\0\0\0\x01\0\0\0\x09ab").expect_err("cut"),
            "failed to extract message 0 size: input data ended unexpectedly"
        );
    }
}
