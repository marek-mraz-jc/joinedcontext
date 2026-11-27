use std::time::Duration;

use async_nats::jetstream::{self, consumer};
use async_trait::async_trait;
use ekiben_core::config::parse_duration_str;
use ekiben_core::{
    AckResponder, Batch, ComponentConfig, CoreError, Input, Part, Registry, StreamContext,
};
use futures::stream::BoxStream;
use futures::StreamExt;
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
    subject_given: bool,
    connected: bool,
    // tokio Mutex: `Input` must be Sync and the message streams are not.
    live: Option<tokio::sync::Mutex<BoxStream<'static, Result<jetstream::Message, String>>>>,
}

impl JetStreamInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let conn_details = NatsConnectionDetails::from_json(
            &conf.config,
            ctx.org.clone(),
            ctx.file_root.as_deref(),
        )?;
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

        // Bound without a subject: the consumer's own filter is checked at connect.
        let subject = match subject_opt {
            Some(raw) => ctx.org.validate_nats_subject(raw)?,
            None => ">".to_string(),
        };

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
        if conn_details.is_in_memory() {
            crate::broker_mem::check_mem_cap("max_ack_pending", max_ack_pending)?;
        }

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
            subject_given: subject_opt.is_some(),
            connected: false,
            live: None,
        })
    }
}

#[async_trait]
impl Input for JetStreamInput {
    fn input_type(&self) -> &'static str {
        "nats_jetstream"
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
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
        let client = self.conn_details.connect_live(&ctx.label).await?;
        self.live = Some(tokio::sync::Mutex::new(
            self.subscribe_live(client, ctx).await?,
        ));
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

        if let Some(live) = self.live.as_mut() {
            let mut shut_rx = ctx.shutdown_rx.clone();
            let live = live.get_mut();
            let next = tokio::select! {
                m = live.next() => m,
                _ = shut_rx.changed() => return Err(CoreError::EndOfInput),
            };
            return match next {
                Some(Ok(msg)) => live_part(msg),
                Some(Err(e)) => {
                    tracing::warn!(label = %ctx.label, "nats_jetstream read failed: {e}");
                    self.live = None;
                    self.connected = false;
                    Err(CoreError::NotConnected)
                }
                None => {
                    self.live = None;
                    self.connected = false;
                    Err(CoreError::NotConnected)
                }
            };
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
                if !delivered.domain.is_empty() {
                    part.meta_set_str("nats_domain", delivered.domain);
                }
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
        self.live = None;
        Ok(())
    }
}

impl JetStreamInput {
    /// Finds or creates the consumer as Bento's `nats_jetstream` does (`nats.go` Subscribe,
    /// QueueSubscribe, PullSubscribe with Bind/BindStream/Durable) and returns its messages.
    /// A new durable without `queue` is a pull consumer, so several instances share it; with
    /// `queue` it is a push consumer with that deliver group (durable defaults to the queue
    /// name), the Bento scale-out pattern. An existing consumer is used as it is.
    async fn subscribe_live(
        &self,
        client: async_nats::Client,
        ctx: &StreamContext,
    ) -> Result<BoxStream<'static, Result<jetstream::Message, String>>, CoreError> {
        let js = jetstream::new(client.clone());
        let user_subject = self.subject_given.then_some(self.subject.as_str());
        let stream_name = match (&self.stream, user_subject) {
            (Some(s), _) => s.clone(),
            (None, Some(subj)) => js.stream_by_subject(subj).await.map_err(|e| {
                CoreError::other(format!("no JetStream stream holds subject '{subj}': {e}"))
            })?,
            (None, None) => {
                return Err(CoreError::config(
                    "bind with only durable needs subject or stream to find the stream",
                ))
            }
        };
        let stream = js.get_stream(&stream_name).await.map_err(|e| {
            CoreError::other(format!(
                "failed to open JetStream stream '{stream_name}': {e}"
            ))
        })?;
        let durable = self.durable.clone().or_else(|| self.queue.clone());

        let existing = match &durable {
            Some(name) => stream.consumer_info(name).await.ok(),
            None => None,
        };
        if self.bind && self.durable.is_some() && existing.is_none() {
            return Err(CoreError::other(format!(
                "bind: consumer '{}' does not exist on stream '{stream_name}'",
                self.durable.as_deref().unwrap_or_default()
            )));
        }

        // An organization reads only subjects under its prefix: a bound consumer's filter, or
        // the stream's subjects when it has none, must pass the prefix check too.
        let filters: Vec<String> = match &existing {
            Some(info) if !info.config.filter_subject.is_empty() => {
                vec![info.config.filter_subject.clone()]
            }
            Some(info) if !info.config.filter_subjects.is_empty() => {
                info.config.filter_subjects.clone()
            }
            Some(_) => Vec::new(),
            None => user_subject.map(String::from).into_iter().collect(),
        };
        // get_stream fetched the stream's info a moment ago: its subjects are current.
        let scope = if filters.is_empty() {
            stream.cached_info().config.subjects.clone()
        } else {
            filters
        };
        for subject in &scope {
            ctx.org.validate_nats_subject(subject)?;
        }

        let deliver_policy = match self.deliver.as_str() {
            "last" => consumer::DeliverPolicy::Last,
            "last_per_subject" => consumer::DeliverPolicy::LastPerSubject,
            "new" => consumer::DeliverPolicy::New,
            _ => consumer::DeliverPolicy::All,
        };
        let filter_subject = user_subject.unwrap_or_default().to_string();
        let max_ack_pending = i64::try_from(self.max_ack_pending).unwrap_or(i64::MAX);

        // A pull consumer is always durable: a bound consumer without a deliver subject, or a
        // new durable without a queue group. Everything else is a push consumer.
        let pull_durable = durable.as_ref().filter(|_| match &existing {
            Some(info) => info.config.deliver_subject.is_none(),
            None => self.queue.is_none(),
        });
        if let Some(name) = pull_durable {
            let config = consumer::pull::Config {
                durable_name: Some(name.clone()),
                filter_subject,
                deliver_policy,
                ack_policy: consumer::AckPolicy::Explicit,
                ack_wait: self.ack_wait,
                max_ack_pending,
                ..Default::default()
            };
            let consumer: consumer::PullConsumer = if existing.is_some() {
                stream.get_consumer(name).await.map_err(consumer_failed)?
            } else {
                // Replicas starting together race to create the durable: the loser uses it.
                match stream.create_consumer(config).await {
                    Ok(c) => c,
                    Err(_) => stream.get_consumer(name).await.map_err(consumer_failed)?,
                }
            };
            // Each pull asks for at most max_ack_pending (cap 200) messages, so instances on
            // one durable split the backlog instead of the first one prefetching all of it.
            let messages = consumer
                .stream()
                .max_messages_per_batch(self.max_ack_pending.clamp(1, 200))
                .messages()
                .await
                .map_err(consumer_failed)?;
            Ok(messages.map(|m| m.map_err(|e| e.to_string())).boxed())
        } else {
            let config = consumer::push::Config {
                deliver_subject: client.new_inbox(),
                deliver_group: self.queue.clone(),
                durable_name: durable.clone(),
                filter_subject,
                deliver_policy,
                ack_policy: consumer::AckPolicy::Explicit,
                ack_wait: self.ack_wait,
                max_ack_pending,
                ..Default::default()
            };
            let consumer: consumer::PushConsumer = match (&existing, &durable) {
                (Some(_), Some(name)) => {
                    stream.get_consumer(name).await.map_err(consumer_failed)?
                }
                // Replicas starting together race to create the durable: the loser uses it.
                (None, Some(name)) => match stream.create_consumer(config).await {
                    Ok(c) => c,
                    Err(_) => stream.get_consumer(name).await.map_err(consumer_failed)?,
                },
                _ => stream
                    .create_consumer(config)
                    .await
                    .map_err(consumer_failed)?,
            };
            let messages = consumer.messages().await.map_err(consumer_failed)?;
            Ok(messages.map(|m| m.map_err(|e| e.to_string())).boxed())
        }
    }
}

/// A consumer the server would not create, open or start, named.
fn consumer_failed(e: impl std::fmt::Display) -> CoreError {
    CoreError::other(format!("failed to create JetStream consumer: {e}"))
}

/// One JetStream message as a Bento part: the same metadata keys, ack = Ack, nack = Nak.
fn live_part(msg: jetstream::Message) -> Result<(Batch, AckResponder), CoreError> {
    let mut part = Part::new(msg.message.payload.clone());
    part.meta_set_str("nats_subject", msg.message.subject.to_string());
    if let Ok(info) = msg.info() {
        part.meta_set_str("nats_sequence_stream", info.stream_sequence.to_string());
        part.meta_set_str("nats_sequence_consumer", info.consumer_sequence.to_string());
        part.meta_set_str("nats_num_delivered", info.delivered.to_string());
        part.meta_set_str("nats_num_pending", info.pending.to_string());
        // Bento sets nats_domain only when the server has a domain.
        if let Some(domain) = info.domain.filter(|d| !d.is_empty()) {
            part.meta_set_str("nats_domain", domain.to_string());
        }
        part.meta_set_str(
            "nats_timestamp_unix_nano",
            info.published.unix_timestamp_nanos().to_string(),
        );
    }
    if let Some(headers) = msg.message.headers.as_ref() {
        for (k, v) in headers.iter() {
            if let Some(first) = v.first().filter(|v| !v.as_str().is_empty()) {
                part.meta_set_str(AsRef::<str>::as_ref(k), first.as_str());
            }
        }
    }
    let ack = AckResponder::from_fn(move |err_opt| {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("nats_jetstream: no runtime to send the ack; the server redelivers");
            return;
        };
        rt.spawn(async move {
            let res = if err_opt.is_none() {
                msg.ack().await
            } else {
                msg.ack_with(jetstream::AckKind::Nak(None)).await
            };
            if let Err(e) = res {
                tracing::warn!("nats_jetstream ack failed, the server redelivers: {e}");
            }
        });
    });
    Ok((vec![part], ack))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);
    impl Write for Logs {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("logs").extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Logs {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().expect("logs")).into_owned()
        }
    }

    #[test]
    fn the_log_writer_keeps_what_it_is_given_across_a_flush() {
        let mut logs = Logs::default();
        logs.write_all(b"one ").expect("write");
        logs.flush().expect("flush");
        logs.write_all(b"two").expect("write");
        assert_eq!(logs.text(), "one two");
    }

    /// A message with an ADR-15 v2 reply subject carries its domain into `nats_domain`; `_` means
    /// none. An ack that cannot be sent (no reply subject) is logged; the server redelivers.
    #[tokio::test(flavor = "current_thread")]
    async fn a_v2_reply_sets_the_domain_and_a_failed_ack_is_logged() {
        let logs = Logs::default();
        let w = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || w.clone())
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let client = async_nats::ConnectOptions::new()
            .retry_on_initial_connect()
            .connect("nats://127.0.0.1:1")
            .await
            .expect("a lazy client needs no server");
        let msg = |reply: Option<&str>| jetstream::Message {
            message: async_nats::Message {
                subject: "events.a".into(),
                reply: reply.map(Into::into),
                payload: bytes::Bytes::from_static(b"x"),
                headers: None,
                status: None,
                description: None,
                length: 1,
            },
            context: jetstream::new(client.clone()),
        };
        let ts = "1790230470084404000";
        let (batch, ack) = live_part(msg(Some(&format!(
            "$JS.ACK.hub.ACC.events.durable.2.10.7.{ts}.3"
        ))))
        .expect("part");
        let part = &batch[0];
        assert_eq!(part.meta_get_str("nats_domain"), "hub");
        assert_eq!(part.meta_get_str("nats_sequence_stream"), "10");
        assert_eq!(part.meta_get_str("nats_num_pending"), "3");
        let (batch, _) = live_part(msg(Some(&format!(
            "$JS.ACK._.ACC.events.durable.2.10.7.{ts}.3"
        ))))
        .expect("part");
        assert!(batch[0].meta_get("nats_domain").is_none());

        drop(ack);
        // No reply subject: no JetStream info, and the ack has nowhere to go.
        let (batch, ack) = live_part(msg(None)).expect("part");
        assert_eq!(batch[0].meta_get_str("nats_subject"), "events.a");
        assert!(batch[0].meta_get("nats_sequence_stream").is_none());
        ack.ack();
        for _ in 0..200 {
            if logs.text().contains("ack failed") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            logs.text()
                .contains("nats_jetstream ack failed, the server redelivers"),
            "{}",
            logs.text()
        );
    }
}
