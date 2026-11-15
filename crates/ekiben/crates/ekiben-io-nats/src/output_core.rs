use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ekiben_bloblang::FieldExpression;
use ekiben_core::config::{extract_metadata_filter, parse_duration_str, MetadataFilter};
use ekiben_core::{
    Batch, ComponentConfig, CoreError, Output, PartsView, ProcContext, Processor, Registry,
    StreamContext,
};
use serde_json::Value as JsonValue;
use tokio::sync::Mutex as AsyncMutex;

use crate::broker_mem::InMemoryNatsCluster;
use crate::connection::NatsConnectionDetails;

pub fn register(registry: &Registry) {
    registry.register_output("nats", |conf, ctx| {
        Ok(Arc::new(NatsCoreOutput::from_config(conf, ctx)?))
    });

    registry.register_processor("nats_request_reply", |conf, ctx| {
        Ok(Arc::new(NatsRequestReplyProcessor::from_config(conf, ctx)?))
    });
}

/// Implements `nats` core output (`internal/impl/nats/output.go`).
pub struct NatsCoreOutput {
    conn_details: NatsConnectionDetails,
    subject: FieldExpression,
    headers: HashMap<String, FieldExpression>,
    meta_filter: MetadataFilter,
    live_client: AsyncMutex<Option<async_nats::Client>>,
}

impl NatsCoreOutput {
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
            .ok_or_else(|| CoreError::config("nats output requires a 'subject' field"))?;
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

        Ok(Self {
            conn_details,
            subject,
            headers,
            meta_filter,
            live_client: AsyncMutex::new(None),
        })
    }
}

#[async_trait]
impl Output for NatsCoreOutput {
    fn output_type(&self) -> &'static str {
        "nats"
    }

    async fn connect(&self, ctx: &StreamContext) -> Result<(), CoreError> {
        if self.conn_details.is_in_memory() {
            return Ok(());
        }
        let mut guard = self.live_client.lock().await;
        if guard.is_none() {
            let client = self.conn_details.connect_live(&ctx.label).await?;
            *guard = Some(client);
        }
        Ok(())
    }

    async fn write_batch(&self, ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        let view = PartsView(&batch);

        for (i, part) in batch.iter().enumerate() {
            let raw_subj = self
                .subject
                .resolve_string(i, &view)
                .map_err(|e| CoreError::other(format!("subject interpolation error: {e}")))?;
            let subject = ctx.org.validate_nats_subject(&raw_subj)?;

            let mut hdrs = BTreeMap::new();
            for (k, expr) in &self.headers {
                let v = expr.resolve_string(i, &view).map_err(|e| {
                    CoreError::other(format!("header {k} interpolation error: {e}"))
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
                cluster.publish_core(subject, None, hdrs, part.as_bytes_ref());
            } else {
                let mut guard = self.live_client.lock().await;
                if guard.is_none() {
                    *guard = Some(self.conn_details.connect_live(&ctx.label).await?);
                }
                let client = guard.as_ref().unwrap();
                let mut nats_hdrs = async_nats::HeaderMap::new();
                for (k, v) in hdrs {
                    nats_hdrs.insert(k.as_str(), v.as_str());
                }
                client
                    .publish_with_headers(subject, nats_hdrs, part.as_bytes_ref())
                    .await
                    .map_err(|e| CoreError::other(format!("nats publish failed: {e}")))?;
            }
        }
        Ok(())
    }
}

/// Implements `nats_request_reply` processor (`internal/impl/nats/processor_request_reply.go`).
pub struct NatsRequestReplyProcessor {
    conn_details: NatsConnectionDetails,
    subject: FieldExpression,
    headers: HashMap<String, FieldExpression>,
    meta_filter: MetadataFilter,
    inbox_prefix: String,
    timeout: Duration,
    live_client: AsyncMutex<Option<async_nats::Client>>,
}

impl NatsRequestReplyProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
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
            .ok_or_else(|| CoreError::config("nats_request_reply requires a 'subject' field"))?;
        let subject = ctx.blobl_env.compile_field(subj_str)?;

        let inbox_prefix = obj
            .get("inbox_prefix")
            .and_then(JsonValue::as_str)
            .unwrap_or("_INBOX")
            .to_string();

        let timeout = match obj.get("timeout").and_then(JsonValue::as_str) {
            Some(s) => parse_duration_str(s)?,
            None => Duration::from_secs(3),
        };

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

        Ok(Self {
            conn_details,
            subject,
            headers,
            meta_filter,
            inbox_prefix,
            timeout,
            live_client: AsyncMutex::new(None),
        })
    }

    /// Sends one request on a live connection and waits for its reply: a private inbox under
    /// `inbox_prefix` is subscribed before the request goes, so no reply is missed.
    async fn request_live(
        &self,
        ctx: &ProcContext,
        subject: String,
        hdrs: BTreeMap<String, String>,
        payload: bytes::Bytes,
    ) -> Result<async_nats::Message, String> {
        use futures::StreamExt;
        let client = {
            let mut guard = self.live_client.lock().await;
            match guard.as_ref() {
                Some(c) => c.clone(),
                None => {
                    let c = self
                        .conn_details
                        .connect_live(&ctx.label)
                        .await
                        .map_err(|e| e.to_string())?;
                    *guard = Some(c.clone());
                    c
                }
            }
        };
        let token = client.new_inbox();
        let token = token.rsplit('.').next().unwrap_or(&token);
        let inbox = format!("{}.{token}", self.inbox_prefix);
        let mut sub = client
            .subscribe(inbox.clone())
            .await
            .map_err(|e| format!("nats subscribe to the reply inbox failed: {e}"))?;
        let mut nats_hdrs = async_nats::HeaderMap::new();
        for (k, v) in &hdrs {
            nats_hdrs.insert(k.as_str(), v.as_str());
        }
        client
            .publish_with_reply_and_headers(subject, inbox, nats_hdrs, payload)
            .await
            .map_err(|e| format!("nats request publish failed: {e}"))?;
        match tokio::time::timeout(self.timeout, sub.next()).await {
            Ok(Some(msg)) if msg.status == Some(async_nats::StatusCode::NO_RESPONDERS) => {
                Err("nats: no responders available for request".to_string())
            }
            Ok(Some(msg)) => Ok(msg),
            Ok(None) => Err("nats: the connection closed before the reply".to_string()),
            Err(_) => Err("nats request_reply timed out".to_string()),
        }
    }
}

#[async_trait]
impl Processor for NatsRequestReplyProcessor {
    fn processor_type(&self) -> &'static str {
        "nats_request_reply"
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
        let mut out = Vec::with_capacity(batch.len());

        for (i, part) in batch.iter().enumerate() {
            let mut p = part.shallow_copy();
            let raw_subj = match self.subject.resolve_string(i, &view) {
                Ok(s) => s,
                Err(e) => {
                    ctx.on_error(&mut p, e.to_string());
                    out.push(p);
                    continue;
                }
            };
            let subject = ctx.org.validate_nats_subject(&raw_subj)?;

            let mut hdrs = BTreeMap::new();
            for (k, expr) in &self.headers {
                if let Ok(v) = expr.resolve_string(i, &view) {
                    hdrs.insert(k.clone(), v);
                }
            }
            for (k, v) in p.meta_all() {
                if self.meta_filter.matches(k) {
                    hdrs.insert(k.clone(), v.to_string_coerced());
                }
            }

            if self.conn_details.is_in_memory() {
                let cluster = InMemoryNatsCluster::get(&self.conn_details.cluster_name());
                let reply_subj = format!("{}.{}", self.inbox_prefix, i);
                let mut reply_rx = cluster.subscribe_core(&reply_subj, None, 1);
                cluster.publish_core(subject, Some(reply_subj), hdrs, p.as_bytes_ref());
                match tokio::time::timeout(self.timeout, reply_rx.recv()).await {
                    Ok(Some(reply_msg)) => {
                        p.set_bytes(reply_msg.data);
                        for (k, v) in reply_msg.headers {
                            p.meta_set_str(k, v);
                        }
                    }
                    _ => {
                        ctx.on_error(&mut p, "nats request_reply timed out");
                    }
                }
            } else {
                match self
                    .request_live(ctx, subject.to_string(), hdrs, p.as_bytes_ref())
                    .await
                {
                    Ok(reply) => {
                        p.set_bytes(reply.payload);
                        if let Some(h) = reply.headers {
                            for (k, vs) in h.iter() {
                                if let Some(v) = vs.first() {
                                    p.meta_set_str(k.to_string(), v.to_string());
                                }
                            }
                        }
                    }
                    Err(e) => ctx.on_error(&mut p, e),
                }
            }
            out.push(p);
        }

        Ok(vec![out])
    }
}
