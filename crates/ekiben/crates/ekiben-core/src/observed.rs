//! Bento's automatic input and output metrics (`internal/component/input/auto_observed.go`
//! semantics as `input_received`, `input_connection_*`, `input_latency_ns`;
//! `internal/component/output/async_writer.go` as `output_sent`, `output_batch_sent`,
//! `output_error`, `output_latency_ns`, `output_connection_*`). The registry wraps every input and
//! output it builds, so broker, fallback and switch children are observed with their own path.

use std::time::Instant;

use async_trait::async_trait;

use crate::error::CoreError;
use crate::message::Batch;
use crate::prometheus::{Counter, Namespaced, Timer};
use crate::traits::{Input, Output, StreamContext};
use crate::transaction::AckResponder;

struct InputMetrics {
    received: Counter,
    connection_up: Counter,
    connection_failed: Counter,
    connection_lost: Counter,
    latency: Timer,
}

impl InputMetrics {
    fn new(ns: Option<&Namespaced>) -> Self {
        match ns {
            Some(ns) => Self {
                received: ns.counter("input_received"),
                connection_up: ns.counter("input_connection_up"),
                connection_failed: ns.counter("input_connection_failed"),
                connection_lost: ns.counter("input_connection_lost"),
                latency: ns.timer("input_latency_ns"),
            },
            None => Self {
                received: Counter::noop(),
                connection_up: Counter::noop(),
                connection_failed: Counter::noop(),
                connection_lost: Counter::noop(),
                latency: Timer::noop(),
            },
        }
    }
}

/// An input with Bento's input metrics, labelled with its `label` and config `path`.
pub struct ObservedInput {
    inner: Box<dyn Input>,
    metrics: InputMetrics,
}

impl ObservedInput {
    /// `ns` is the component's namespace (`None` when the stream has no metrics registry).
    pub fn new(inner: Box<dyn Input>, ns: Option<Namespaced>) -> Self {
        Self {
            inner,
            metrics: InputMetrics::new(ns.as_ref()),
        }
    }
}

#[async_trait]
impl Input for ObservedInput {
    fn input_type(&self) -> &'static str {
        self.inner.input_type()
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        match self.inner.connect(ctx).await {
            Ok(()) => {
                self.metrics.connection_up.incr(1);
                Ok(())
            }
            Err(e) => {
                self.metrics.connection_failed.incr(1);
                Err(e)
            }
        }
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        match self.inner.read_batch(ctx).await {
            Ok((batch, ack)) => {
                self.metrics.received.incr(batch.len() as u64);
                let latency = self.metrics.latency.clone();
                let read_at = Instant::now();
                let wrapped = AckResponder::from_fn(move |res| {
                    latency.timing(u64::try_from(read_at.elapsed().as_nanos()).unwrap_or(u64::MAX));
                    ack.respond(res);
                });
                Ok((batch, wrapped))
            }
            Err(CoreError::NotConnected) => {
                self.metrics.connection_lost.incr(1);
                Err(CoreError::NotConnected)
            }
            Err(e) => Err(e),
        }
    }

    fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        self.inner.close().await
    }
}

struct OutputMetrics {
    sent: Counter,
    batch_sent: Counter,
    error: Counter,
    latency: Timer,
    connection_up: Counter,
    connection_failed: Counter,
    connection_lost: Counter,
}

impl OutputMetrics {
    fn new(ns: Option<&Namespaced>) -> Self {
        match ns {
            Some(ns) => Self {
                sent: ns.counter("output_sent"),
                batch_sent: ns.counter("output_batch_sent"),
                error: ns.counter("output_error"),
                latency: ns.timer("output_latency_ns"),
                connection_up: ns.counter("output_connection_up"),
                connection_failed: ns.counter("output_connection_failed"),
                connection_lost: ns.counter("output_connection_lost"),
            },
            None => Self {
                sent: Counter::noop(),
                batch_sent: Counter::noop(),
                error: Counter::noop(),
                latency: Timer::noop(),
                connection_up: Counter::noop(),
                connection_failed: Counter::noop(),
                connection_lost: Counter::noop(),
            },
        }
    }
}

/// An output with Bento's output metrics, labelled with its `label` and config `path`.
pub struct ObservedOutput {
    inner: std::sync::Arc<dyn Output>,
    metrics: OutputMetrics,
}

impl ObservedOutput {
    pub fn new(inner: std::sync::Arc<dyn Output>, ns: Option<Namespaced>) -> Self {
        Self {
            inner,
            metrics: OutputMetrics::new(ns.as_ref()),
        }
    }
}

#[async_trait]
impl Output for ObservedOutput {
    fn output_type(&self) -> &'static str {
        self.inner.output_type()
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    async fn connect(&self, ctx: &StreamContext) -> Result<(), CoreError> {
        match self.inner.connect(ctx).await {
            Ok(()) => {
                self.metrics.connection_up.incr(1);
                Ok(())
            }
            Err(e) => {
                self.metrics.connection_failed.incr(1);
                Err(e)
            }
        }
    }

    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        let parts = batch.len() as u64;
        let started = Instant::now();
        let res = self.inner.write_batch(ctx, batch).await;
        self.metrics
            .latency
            .timing(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
        match &res {
            Ok(()) => {
                self.metrics.sent.incr(parts);
                self.metrics.batch_sent.incr(1);
            }
            Err(CoreError::NotConnected) => {
                self.metrics.connection_lost.incr(1);
                self.metrics.error.incr(1);
            }
            Err(_) => self.metrics.error.incr(1),
        }
        res
    }

    fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    async fn close(&self) -> Result<(), CoreError> {
        self.inner.close().await
    }
}
