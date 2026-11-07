use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::{FieldExpression, Value};
use ekiben_core::{
    Batch, ComponentConfig, CoreError, Output, Part, PartsView, ProcContext, Processor, Registry,
    StreamContext,
};
use serde_json::Value as JsonValue;

use crate::broker_mem::{InMemoryNatsCluster, KvEntry};
use crate::connection::NatsConnectionDetails;

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
    conn_details: NatsConnectionDetails,
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
            conn_details,
            bucket,
            operation,
            key,
            revision,
        })
    }

    fn effective_bucket(&self, ctx: &ProcContext) -> String {
        format!("{}::{}", ctx.org_id(), self.bucket)
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
        let cluster = InMemoryNatsCluster::get(&self.conn_details.cluster_name());
        let eff_bucket = self.effective_bucket(ctx);
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

            match self.operation.as_str() {
                "get" => match cluster.kv_get(&eff_bucket, &key_str) {
                    Ok(entry) => {
                        self.annotate_from_entry(&mut m, &entry);
                        m.set_bytes(entry.value);
                    }
                    Err(e) => ctx.on_error(&mut m, e.to_string()),
                },
                "get_revision" => {
                    let rev = self
                        .revision
                        .as_ref()
                        .and_then(|r| r.resolve_string(i, &view).ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    match cluster.kv_get_revision(&eff_bucket, &key_str, rev) {
                        Ok(entry) => {
                            self.annotate_from_entry(&mut m, &entry);
                            m.set_bytes(entry.value);
                        }
                        Err(e) => ctx.on_error(&mut m, e.to_string()),
                    }
                }
                "create" => {
                    let val = m.as_bytes_ref();
                    match cluster.kv_create(&eff_bucket, &key_str, val) {
                        Ok(rev) => self.annotate_op(&mut m, &key_str, rev, "KeyValuePutOp"),
                        Err(e) => ctx.on_error(&mut m, e.to_string()),
                    }
                }
                "put" => {
                    let val = m.as_bytes_ref();
                    let rev = cluster.kv_put(&eff_bucket, &key_str, val);
                    self.annotate_op(&mut m, &key_str, rev, "KeyValuePutOp");
                }
                "update" => {
                    let rev = self
                        .revision
                        .as_ref()
                        .and_then(|r| r.resolve_string(i, &view).ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    let val = m.as_bytes_ref();
                    match cluster.kv_update(&eff_bucket, &key_str, val, rev) {
                        Ok(new_rev) => self.annotate_op(&mut m, &key_str, new_rev, "KeyValuePutOp"),
                        Err(e) => ctx.on_error(&mut m, e.to_string()),
                    }
                }
                "delete" => {
                    let rev = cluster.kv_delete(&eff_bucket, &key_str);
                    self.annotate_op(&mut m, &key_str, rev, "KeyValueDeleteOp");
                }
                "purge" => {
                    let rev = cluster.kv_purge(&eff_bucket, &key_str);
                    self.annotate_op(&mut m, &key_str, rev, "KeyValuePurgeOp");
                }
                "history" => {
                    let entries = cluster.kv_history(&eff_bucket, &key_str);
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
                    let keys = cluster.kv_keys(&eff_bucket, &key_str);
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
    conn_details: NatsConnectionDetails,
    bucket: String,
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
            conn_details,
            bucket,
            key,
        })
    }
}

#[async_trait]
impl Output for NatsKvOutput {
    fn output_type(&self) -> &'static str {
        "nats_kv"
    }

    async fn write_batch(&self, ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        let view = PartsView(&batch);
        let cluster = InMemoryNatsCluster::get(&self.conn_details.cluster_name());
        let eff_bucket = format!("{}::{}", ctx.org_id(), self.bucket);

        for (i, part) in batch.iter().enumerate() {
            let k = self
                .key
                .resolve_string(i, &view)
                .map_err(|e| CoreError::other(e.to_string()))?;
            cluster.kv_put(&eff_bucket, &k, part.as_bytes_ref());
        }
        Ok(())
    }
}
