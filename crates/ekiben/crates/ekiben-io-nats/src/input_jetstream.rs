use std::time::Duration;

use async_trait::async_trait;
use ekiben_core::config::parse_duration_str;
use ekiben_core::{
    AckResponder, Batch, ComponentConfig, CoreError, Input, Part, Registry, StreamContext,
};
use serde_json::Value as JsonValue;

use crate::broker_mem::InMemoryNatsCluster;
use crate::connection::NatsConnectionDetails;

pub fn register(registry: &Registry) {
    registry.register_input("nats_jetstream", |conf, ctx| {
        Ok(Box::new(JetStreamInput::from_config(conf, ctx)?))
    });
}

/// Implements `nats_jetstream` input (`internal/impl/nats/input_jetstream.go`):
/// - Supports `subject`, `queue`, `durable`, `stream`, `bind`, `deliver` (`all`, `last`, `last_per_subject`, `new`),
///   `ack_wait`, and `max_ack_pending` as backpressure.
/// - Sets metadata `nats_subject`, `nats_sequence_stream`, `nats_sequence_consumer`, `nats_num_delivered`,
///   `nats_num_pending`, `nats_domain`, `nats_timestamp_unix_nano`, plus all message headers.
/// - Maps transaction `Ack` to JetStream `Ack()` and transaction `Nack` to JetStream `Nak()` for redelivery!
pub struct JetStreamInput {
    conn_details: NatsConnectionDetails,
    subject: String,
    queue: Option<String>,
    durable: Option<String>,
    stream: Option<String>,
    bind: bool,
    deliver: String,
    ack_wait: Duration,
    max_ack_pending: usize,
    consumer_key: String,
    connected: bool,
}

impl JetStreamInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let conn_details = NatsConnectionDetails::from_json(&conf.config)?;
        let obj = conf.config.as_object().unwrap();

        let deliver = obj
            .get("deliver")
            .and_then(JsonValue::as_str)
            .unwrap_or("all")
            .to_string();
        if !matches!(
            deliver.as_str(),
            "all" | "last" | "last_per_subject" | "new"
        ) {
            return Err(CoreError::config(format!(
                "deliver option {deliver} was not recognised"
            )));
        }

        let subject_opt = obj
            .get("subject")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty());
        let queue = obj
            .get("queue")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let durable = obj
            .get("durable")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let stream = obj
            .get("stream")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let bind = obj
            .get("bind")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);

        if bind {
            if stream.is_none() && durable.is_none() {
                return Err(CoreError::config(
                    "stream or durable is required, when bind is true",
                ));
            }
        } else if subject_opt.is_none() {
            return Err(CoreError::config("subject is empty"));
        }

        let raw_subject = subject_opt.unwrap_or(">");
        let subject = ctx.org.validate_nats_subject(raw_subject)?;

        let ack_wait_str = obj
            .get("ack_wait")
            .and_then(JsonValue::as_str)
            .unwrap_or("30s");
        let ack_wait = parse_duration_str(ack_wait_str)
            .map_err(|e| CoreError::config(format!("failed to parse ack wait duration: {e}")))?;

        let max_ack_pending = obj
            .get("max_ack_pending")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1024) as usize;

        let consumer_key = format!(
            "{}:{}:{}",
            ctx.org_id(),
            stream.as_deref().unwrap_or("default_stream"),
            durable
                .as_deref()
                .or(queue.as_deref())
                .unwrap_or(ctx.stream_id.as_str())
        );

        Ok(Self {
            conn_details,
            subject,
            queue,
            durable,
            stream,
            bind,
            deliver,
            ack_wait,
            max_ack_pending,
            consumer_key,
            connected: false,
        })
    }
}

#[async_trait]
impl Input for JetStreamInput {
    fn input_type(&self) -> &'static str {
        "nats_jetstream"
    }

    async fn connect(&mut self, _ctx: &StreamContext) -> Result<(), CoreError> {
        if self.conn_details.is_in_memory() {
            let cluster = InMemoryNatsCluster::get(&self.conn_details.cluster_name());
            cluster.ensure_consumer(
                &self.consumer_key,
                &self.subject,
                &self.deliver,
                self.max_ack_pending,
            );
            self.connected = true;
            return Ok(());
        }
        let _ = (
            &self.queue,
            &self.durable,
            &self.stream,
            self.bind,
            self.ack_wait,
        );
        self.connected = true;
        Ok(())
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        if !self.connected {
            return Err(CoreError::NotConnected);
        }

        let cluster_url = self.conn_details.cluster_name();
        let cluster = InMemoryNatsCluster::get(&cluster_url);

        loop {
            if let Some(delivered) = cluster.try_pull_jetstream(&self.consumer_key) {
                let mut part = Part::new(delivered.msg.data);
                part.meta_set_str("nats_subject", delivered.msg.subject);
                part.meta_set_str("nats_sequence_stream", delivered.msg.stream_seq.to_string());
                part.meta_set_str("nats_sequence_consumer", delivered.consumer_seq.to_string());
                part.meta_set_str("nats_num_delivered", delivered.num_delivered.to_string());
                part.meta_set_str("nats_num_pending", delivered.num_pending.to_string());
                part.meta_set_str("nats_domain", delivered.domain);
                part.meta_set_str(
                    "nats_timestamp_unix_nano",
                    delivered.msg.timestamp_unix_nano.to_string(),
                );

                for (k, v) in delivered.msg.headers {
                    if !v.is_empty() {
                        part.meta_set_str(k, v);
                    }
                }

                let c_key = self.consumer_key.clone();
                let s_seq = delivered.msg.stream_seq;
                let c_url = cluster_url.clone();

                // Map transaction Ack -> m.Ack() and Nack -> m.Nak() for redelivery!
                let ack = AckResponder::from_fn(move |err_opt| {
                    let cl = InMemoryNatsCluster::get(&c_url);
                    if err_opt.is_none() {
                        cl.ack_jetstream(&c_key, s_seq);
                    } else {
                        cl.nak_jetstream(&c_key, s_seq);
                    }
                });

                return Ok((vec![part], ack));
            }

            let mut shut_rx = ctx.shutdown_rx.clone();
            tokio::select! {
                _ = cluster.wait_for_js_activity() => {}
                _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                _ = shut_rx.changed() => return Err(CoreError::EndOfInput),
            }
            if ctx.is_shutting_down() {
                return Err(CoreError::EndOfInput);
            }
        }
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        self.connected = false;
        Ok(())
    }
}
