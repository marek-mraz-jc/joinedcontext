use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::{FieldExpression, Value};
use ekiben_core::{
    Batch, ComponentConfig, CoreError, Output, Part, PartsView, ProcContext, Processor, Registry,
    StreamContext,
};
use serde_json::Value as JsonValue;

use crate::broker_mem::KvEntry;
use crate::connection::NatsConnectionDetails;
use crate::kv_backend::KvBucket;

pub fn register(registry: &Registry) {
    registry.register_processor("nats_kv", |conf, ctx| {
        Ok(Arc::new(NatsKvProcessor::from_config(conf, ctx)?))
    });

    registry.register_output("nats_kv", |conf, ctx| {
        Ok(Arc::new(NatsKvOutput::from_config(conf, ctx)?))
    });
}

/// Implements `nats_kv` processor (`internal/impl/nats/processor_kv.go`).
/// Supports `get`, `get_revision`, `create`, `put`, `update`, `delete`, `purge`, `history`, `keys`.
pub struct NatsKvProcessor {
    store: KvBucket,
    bucket: String,
    operation: String,
    key: FieldExpression,
    revision: Option<FieldExpression>,
}

impl NatsKvProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let conn_details = NatsConnectionDetails::from_json(&conf.config)?;
        let obj = conf.config.as_object().unwrap();

        let bucket = obj
            .get("bucket")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::config("nats_kv requires a 'bucket' field"))?
            .to_string();

        let operation = obj
            .get("operation")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("nats_kv requires an 'operation' field"))?
            .to_string();

        let key_str = obj
            .get("key")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::config("nats_kv 'key' must be set to a non-empty string"))?;
        let key = ctx.blobl_env.compile_field(key_str)?;

        let revision = match obj.get("revision").and_then(JsonValue::as_str) {
            Some(s) if !s.is_empty() => Some(ctx.blobl_env.compile_field(s)?),
            _ => None,
        };

        if matches!(operation.as_str(), "get_revision" | "update") && revision.is_none() {
            return Err(CoreError::config(format!(
                "'revision' must be set when operation is '{operation}'"
            )));
        }
        if !matches!(operation.as_str(), "get_revision" | "update") && revision.is_some() {
            return Err(CoreError::config(format!(
                "'revision' cannot be set when operation is '{operation}'"
            )));
        }

        Ok(Self {
            store: KvBucket::new(conn_details, &ctx.org, &bucket)?,
            bucket,
            operation,
            key,
            revision,
        })
    }

    fn annotate_from_entry(&self, part: &mut Part, entry: &KvEntry) {
        part.meta_set_str("nats_kv_key", entry.key.clone());
        part.meta_set_str("nats_kv_bucket", self.bucket.clone());
        part.meta_set("nats_kv_revision", Value::Uint(entry.revision));
        part.meta_set("nats_kv_delta", Value::Uint(entry.delta));
        part.meta_set_str("nats_kv_operation", entry.operation);
        part.meta_set_str("nats_kv_created", entry.created.clone());
    }

    fn annotate_op(&self, part: &mut Part, key: &str, rev: u64, op: &str) {
        part.meta_set_str("nats_kv_key", key);
        part.meta_set_str("nats_kv_bucket", self.bucket.clone());
        part.meta_set("nats_kv_revision", Value::Uint(rev));
        part.meta_set_str("nats_kv_operation", op);
    }
}

#[async_trait]
impl Processor for NatsKvProcessor {
    fn processor_type(&self) -> &'static str {
        "nats_kv"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        let view = PartsView(&batch);
        let kv = &self.store;
        let mut out = Vec::with_capacity(batch.len());

        for (i, part) in batch.iter().enumerate() {
            let mut m = part.shallow_copy();
            let key_str = match self.key.resolve_string(i, &view) {
                Ok(k) => k,
                Err(e) => {
                    ctx.on_error(&mut m, e.to_string());
                    out.push(m);
                    continue;
                }
            };

            // Bento's texts: a missing key is nats.go's ErrKeyNotFound, an existing one the
            // full API error.
            let fail = |m: &mut Part, e: CoreError| match e {
                CoreError::KeyNotFound(_) => ctx.on_error(m, "nats: key not found".to_string()),
                CoreError::KeyAlreadyExists(text) => ctx.on_error(m, text),
                other => ctx.on_error(m, other.to_string()),
            };
            match self.operation.as_str() {
                "get" => match kv.get(&key_str).await {
                    Ok(entry) => {
                        self.annotate_from_entry(&mut m, &entry);
                        m.set_bytes(entry.value);
                    }
                    Err(e) => fail(&mut m, e),
                },
                "get_revision" => {
                    let rev = self
                        .revision
                        .as_ref()
                        .and_then(|r| r.resolve_string(i, &view).ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    match kv.get_revision(&key_str, rev).await {
                        Ok(entry) => {
                            self.annotate_from_entry(&mut m, &entry);
                            m.set_bytes(entry.value);
                        }
                        Err(e) => fail(&mut m, e),
                    }
                }
                "create" => {
                    let val = m.as_bytes_ref();
                    match kv.create(&key_str, val).await {
                        Ok(rev) => self.annotate_op(&mut m, &key_str, rev, "KeyValuePutOp"),
                        Err(e) => fail(&mut m, e),
                    }
                }
                "put" => {
                    let val = m.as_bytes_ref();
                    match kv.put(&key_str, val).await {
                        Ok(rev) => self.annotate_op(&mut m, &key_str, rev, "KeyValuePutOp"),
                        Err(e) => fail(&mut m, e),
                    }
                }
                "update" => {
                    let rev = self
                        .revision
                        .as_ref()
                        .and_then(|r| r.resolve_string(i, &view).ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    let val = m.as_bytes_ref();
                    match kv.update(&key_str, val, rev).await {
                        Ok(new_rev) => self.annotate_op(&mut m, &key_str, new_rev, "KeyValuePutOp"),
                        Err(e) => fail(&mut m, e),
                    }
                }
                "delete" => match kv.delete(&key_str).await {
                    Ok(rev) => self.annotate_op(&mut m, &key_str, rev, "KeyValueDeleteOp"),
                    Err(e) => fail(&mut m, e),
                },
                "purge" => match kv.purge(&key_str).await {
                    Ok(rev) => self.annotate_op(&mut m, &key_str, rev, "KeyValuePurgeOp"),
                    Err(e) => fail(&mut m, e),
                },
                "history" => {
                    let entries = match kv.history(&key_str).await {
                        Ok(entries) => entries,
                        Err(e) => {
                            ctx.on_error(&mut m, e.to_string());
                            out.push(m);
                            continue;
                        }
                    };
                    let mut records = Vec::with_capacity(entries.len());
                    for entry in entries {
                        let mut rec = BTreeMap::new();
                        rec.insert("key".to_string(), Value::String(entry.key));
                        rec.insert("value".to_string(), Value::Bytes(entry.value.to_vec()));
                        rec.insert("bucket".to_string(), Value::String(self.bucket.clone()));
                        rec.insert("revision".to_string(), Value::Uint(entry.revision));
                        rec.insert("delta".to_string(), Value::Uint(entry.delta));
                        rec.insert(
                            "operation".to_string(),
                            Value::String(entry.operation.to_string()),
                        );
                        rec.insert("created".to_string(), Value::String(entry.created));
                        records.push(Value::Object(rec.into()));
                    }
                    m.set_structured(Value::Array(records.into()));
                }
                "keys" => {
                    let keys = match kv.keys(&key_str).await {
                        Ok(keys) => keys,
                        Err(e) => {
                            ctx.on_error(&mut m, e.to_string());
                            out.push(m);
                            continue;
                        }
                    };
                    m.set_structured(Value::Array(keys.into_iter().map(Value::String).collect()));
                    m.meta_set_str("nats_kv_bucket", self.bucket.clone());
                }
                other => {
                    ctx.on_error(&mut m, format!("invalid kv operation: {other}"));
                }
            }
            out.push(m);
        }

        Ok(vec![out])
    }
}

/// Implements `nats_kv` output (`internal/impl/nats/output_kv.go`).
pub struct NatsKvOutput {
    store: KvBucket,
    key: FieldExpression,
}

impl NatsKvOutput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let conn_details = NatsConnectionDetails::from_json(&conf.config)?;
        let obj = conf.config.as_object().unwrap();
        let bucket = obj
            .get("bucket")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::config("nats_kv output requires a 'bucket' field"))?
            .to_string();
        let key_str = obj
            .get("key")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::config("nats_kv output requires a 'key' field"))?;
        let key = ctx.blobl_env.compile_field(key_str)?;
        Ok(Self {
            store: KvBucket::new(conn_details, &ctx.org, &bucket)?,
            key,
        })
    }
}

#[async_trait]
impl Output for NatsKvOutput {
    fn output_type(&self) -> &'static str {
        "nats_kv"
    }

    async fn write_batch(&self, _ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        let view = PartsView(&batch);
        for (i, part) in batch.iter().enumerate() {
            let k = self
                .key
                .resolve_string(i, &view)
                .map_err(|e| CoreError::other(e.to_string()))?;
            self.store.put(&k, part.as_bytes_ref()).await?;
        }
        Ok(())
    }
}
