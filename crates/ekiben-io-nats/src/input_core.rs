use std::time::Duration;

use async_trait::async_trait;
use ekiben_core::config::parse_duration_str;
use ekiben_core::{
    AckResponder, Batch, ComponentConfig, CoreError, Input, Part, Registry, StreamContext,
    SyncResponseStore,
};
use futures::StreamExt;
use serde_json::Value as JsonValue;
use tokio::sync::mpsc;

use crate::broker_mem::{InMemoryNatsCluster, MemNatsMsg};
use crate::connection::NatsConnectionDetails;

pub fn register(registry: &Registry) {
    registry.register_input("nats", |conf, ctx| {
        Ok(Box::new(NatsCoreInput::from_config(conf, ctx)?))
    });
}

/// Implements `nats` core input (`internal/impl/nats/input.go`).
pub struct NatsCoreInput {
    conn_details: NatsConnectionDetails,
    subject: String,
    queue: Option<String>,
    prefetch_count: usize,
    nak_delay: Option<Duration>,
    send_ack: bool,
    mem_rx: Option<mpsc::Receiver<MemNatsMsg>>,
    live_sub: Option<async_nats::Subscriber>,
    live_client: Option<async_nats::Client>,
    connected: bool,
}

impl NatsCoreInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let conn_details = NatsConnectionDetails::from_json(&conf.config)?;
        let obj = conf.config.as_object().unwrap();

        let raw_subject = obj
            .get("subject")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::config("nats input requires a 'subject' field"))?;
        let subject = ctx.org.validate_nats_subject(raw_subject)?;

        let queue = obj
            .get("queue")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let prefetch_count = obj
            .get("prefetch_count")
            .and_then(JsonValue::as_i64)
            .unwrap_or(512);
        if prefetch_count < 0 {
            return Err(CoreError::config(
                "prefetch count must be greater than or equal to zero",
            ));
        }

        let nak_delay = match obj.get("nak_delay").and_then(JsonValue::as_str) {
            Some(s) if !s.is_empty() => Some(parse_duration_str(s)?),
            _ => None,
        };

        let send_ack = obj
            .get("send_ack")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true);

        Ok(Self {
            conn_details,
            subject,
            queue,
            prefetch_count: prefetch_count as usize,
            nak_delay,
            send_ack,
            mem_rx: None,
            live_sub: None,
            live_client: None,
            connected: false,
        })
    }
}

#[async_trait]
impl Input for NatsCoreInput {
    fn input_type(&self) -> &'static str {
        "nats"
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        if self.connected {
            return Ok(());
        }
        if self.conn_details.is_in_memory() {
            let cluster = InMemoryNatsCluster::get(&self.conn_details.cluster_name());
            self.mem_rx = Some(cluster.subscribe_core(
                &self.subject,
                self.queue.clone(),
                self.prefetch_count,
            ));
            self.connected = true;
            return Ok(());
        }

        let client = self.conn_details.connect_live(&ctx.label).await?;
        let sub = if let Some(ref q) = self.queue {
            client
                .queue_subscribe(self.subject.clone(), q.clone())
                .await
                .map_err(|e| CoreError::other(format!("nats queue_subscribe failed: {e}")))?
        } else {
            client
                .subscribe(self.subject.clone())
                .await
                .map_err(|e| CoreError::other(format!("nats subscribe failed: {e}")))?
        };
        self.live_client = Some(client);
        self.live_sub = Some(sub);
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

        if let Some(ref mut rx) = self.mem_rx {
            let mut shut_rx = ctx.shutdown_rx.clone();
            let msg = tokio::select! {
                m_opt = rx.recv() => m_opt.ok_or(CoreError::EndOfInput)?,
                _ = shut_rx.changed() => return Err(CoreError::EndOfInput),
            };
            let mut part = Part::new(msg.data);
            part.meta_set_str("nats_subject", msg.subject);
            part.meta_set_str("nats_reply_subject", msg.reply.clone().unwrap_or_default());
            for (k, v) in msg.headers {
                part.meta_set_str(k, v);
            }
            let store = SyncResponseStore::new();
            part.with_sync_response_store(store.clone());

            let reply_subj = msg.reply;
            let cluster_url = self.conn_details.cluster_name();
            let send_ack = self.send_ack;
            let _ = self.nak_delay;

            let ack = AckResponder::from_fn(move |err_opt| {
                if err_opt.is_none() {
                    if let Some(reply) = reply_subj {
                        let cluster = InMemoryNatsCluster::get(&cluster_url);
                        let responses = store.read();
                        if let Some(first_batch) = responses.first() {
                            if let Some(first_part) = first_batch.first() {
                                let mut hdrs = std::collections::BTreeMap::new();
                                for (k, v) in first_part.meta_all() {
                                    hdrs.insert(k.clone(), v.to_string_coerced());
                                }
                                cluster.publish_core(reply, None, hdrs, first_part.as_bytes_ref());
                                return;
                            }
                        }
                        if send_ack {
                            cluster.publish_core(
                                reply,
                                None,
                                Default::default(),
                                bytes::Bytes::from_static(b"+ACK"),
                            );
                        }
                    }
                }
            });

            return Ok((vec![part], ack));
        }

        if let Some(ref mut sub) = self.live_sub {
            let mut shut_rx = ctx.shutdown_rx.clone();
            let nmsg = tokio::select! {
                m_opt = sub.next() => m_opt.ok_or(CoreError::NotConnected)?,
                _ = shut_rx.changed() => return Err(CoreError::EndOfInput),
            };
            let mut part = Part::new(nmsg.payload);
            part.meta_set_str("nats_subject", nmsg.subject.to_string());
            let reply_str = nmsg
                .reply
                .as_ref()
                .map(|r| r.to_string())
                .unwrap_or_default();
            part.meta_set_str("nats_reply_subject", reply_str);
            if let Some(headers) = nmsg.headers {
                for (k, v) in headers.iter() {
                    if let Some(first_val) = v.first() {
                        part.meta_set_str(AsRef::<str>::as_ref(k), first_val.as_str());
                    }
                }
            }
            return Ok((vec![part], AckResponder::noop()));
        }

        Err(CoreError::NotConnected)
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        self.connected = false;
        self.mem_rx = None;
        if let Some(mut sub) = self.live_sub.take() {
            let _ = sub.unsubscribe().await;
        }
        Ok(())
    }
}
