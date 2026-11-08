use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::FieldExpression;
use ekiben_core::config::{extract_metadata_filter, MetadataFilter};
use ekiben_core::{Batch, ComponentConfig, CoreError, Output, PartsView, Registry, StreamContext};
use serde_json::Value as JsonValue;

use crate::broker_mem::InMemoryNatsCluster;
use crate::connection::NatsConnectionDetails;

pub fn register(registry: &Registry) {
    registry.register_output("nats_jetstream", |conf, ctx| {
        Ok(Arc::new(JetStreamOutput::from_config(conf, ctx)?))
    });
}

/// Implements `nats_jetstream` output (`internal/impl/nats/output_jetstream.go`).
pub struct JetStreamOutput {
    conn_details: NatsConnectionDetails,
    subject: FieldExpression,
    headers: HashMap<String, FieldExpression>,
    meta_filter: MetadataFilter,
    max_in_flight: usize,
    live: tokio::sync::Mutex<Option<async_nats::jetstream::Context>>,
}

impl JetStreamOutput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let conn_details = NatsConnectionDetails::from_json(
            &conf.config,
            ctx.org.clone(),
            ctx.file_root.as_deref(),
        )?;
        let obj = conf.config.as_object().unwrap();

        let subj_str = obj
            .get("subject")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::config("nats_jetstream output requires a 'subject' field"))?;
        let subject = ctx.blobl_env.compile_field(subj_str)?;

        let mut headers = HashMap::new();
        if let Some(h_map) = obj.get("headers").and_then(JsonValue::as_object) {
            for (k, v) in h_map {
                if let Some(s) = v.as_str() {
                    headers.insert(k.clone(), ctx.blobl_env.compile_field(s)?);
                }
            }
        }

        let meta_filter = obj
            .get("metadata")
            .map(extract_metadata_filter)
            .unwrap_or_default();

        let max_in_flight = match obj.get("max_in_flight") {
            None => 1024,
            Some(v) => v
                .as_u64()
                .filter(|n| *n > 0)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| CoreError::config("max_in_flight must be a positive integer"))?,
        };

        Ok(Self {
            conn_details,
            subject,
            headers,
            meta_filter,
            max_in_flight,
            live: tokio::sync::Mutex::new(None),
        })
    }
}

#[async_trait]
impl Output for JetStreamOutput {
    fn output_type(&self) -> &'static str {
        "nats_jetstream"
    }

    async fn connect(&self, ctx: &StreamContext) -> Result<(), CoreError> {
        if self.conn_details.is_in_memory() {
            return Ok(());
        }
        let mut guard = self.live.lock().await;
        if guard.is_none() {
            let client = self.conn_details.connect_live(&ctx.label).await?;
            *guard = Some(async_nats::jetstream::new(client));
        }
        Ok(())
    }

    async fn write_batch(&self, ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        let view = PartsView(&batch);
        let mut js: Option<async_nats::jetstream::Context> = None;
        let mut pending = Vec::new();

        for (i, part) in batch.iter().enumerate() {
            let raw_subj = self.subject.resolve_string(i, &view).map_err(|e| {
                CoreError::other(format!(
                    "failed string interpolation on field \"subject\": {e}"
                ))
            })?;
            let subject = ctx.org.validate_nats_subject(&raw_subj)?;

            let mut hdrs = BTreeMap::new();
            for (k, expr) in &self.headers {
                let v = expr.resolve_string(i, &view).map_err(|e| {
                    CoreError::other(format!("failed string interpolation on header {k:?}: {e}"))
                })?;
                hdrs.insert(k.clone(), v);
            }
            for (k, v) in part.meta_all() {
                if self.meta_filter.matches(k) {
                    hdrs.insert(k.clone(), v.to_string_coerced());
                }
            }

            if self.conn_details.is_in_memory() {
                let cluster = InMemoryNatsCluster::get(&self.conn_details.cluster_name());
                cluster.publish_jetstream(subject, hdrs, part.as_bytes_ref());
                continue;
            }
            if js.is_none() {
                let mut guard = self.live.lock().await;
                if guard.is_none() {
                    let client = self.conn_details.connect_live(&ctx.label).await?;
                    *guard = Some(async_nats::jetstream::new(client));
                }
                js = guard.clone();
            }
            let Some(js) = js.as_ref() else {
                return Err(CoreError::NotConnected);
            };
            let mut nats_hdrs = async_nats::HeaderMap::new();
            for (k, v) in hdrs {
                nats_hdrs.insert(k.as_str(), v.as_str());
            }
            let ack = js
                .publish_with_headers(subject, nats_hdrs, part.as_bytes_ref())
                .await
                .map_err(|e| CoreError::other(format!("nats_jetstream publish failed: {e}")))?;
            pending.push(ack);
            if pending.len() >= self.max_in_flight {
                await_acks(&mut pending).await?;
            }
        }
        await_acks(&mut pending).await
    }
}

/// Waits for every publish ack: the batch is acked upstream only once JetStream stored it.
async fn await_acks(
    pending: &mut Vec<async_nats::jetstream::context::PublishAckFuture>,
) -> Result<(), CoreError> {
    for res in
        futures::future::join_all(pending.drain(..).map(std::future::IntoFuture::into_future)).await
    {
        res.map_err(|e| CoreError::other(format!("nats_jetstream publish not acknowledged: {e}")))?;
    }
    Ok(())
}
