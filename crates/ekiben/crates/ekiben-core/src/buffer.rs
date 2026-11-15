//! The stream's buffer between its input and pipeline (`internal/component/buffer/stream.go`):
//! a task feeds every input transaction into the buffer, its ack passed through, and the pipeline
//! reads what the buffer releases. When the input has ended and every transaction it sent is
//! acked, the buffer is told (`end_of_input`), as Bento's async reader waits for its pending acks.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use tokio::sync::watch;

use crate::config::ComponentConfig;
use crate::error::{BatchError, CoreError};
use crate::message::{Batch, Part};
use crate::prometheus::Gauge;
use crate::traits::{Buffer, Input, StreamContext};
use crate::transaction::{AckResponder, Transaction};

pub struct BufferedInput {
    input_type: &'static str,
    inner: Option<Box<dyn Input>>,
    buffer: Arc<dyn Buffer>,
    read_err: Arc<Mutex<Option<String>>>,
    feeder: Option<tokio::task::JoinHandle<()>>,
}

impl BufferedInput {
    pub fn new(inner: Box<dyn Input>, buffer: Arc<dyn Buffer>) -> Self {
        Self {
            input_type: inner.input_type(),
            inner: Some(inner),
            buffer,
            read_err: Arc::new(Mutex::new(None)),
            feeder: None,
        }
    }
}

/// Transactions the input sent that are not acked yet.
#[derive(Default)]
struct Outstanding {
    count: AtomicUsize,
    acked: tokio::sync::Notify,
}

impl Outstanding {
    fn track(self: &Arc<Self>, ack: AckResponder) -> AckResponder {
        self.count.fetch_add(1, Ordering::AcqRel);
        let this = self.clone();
        AckResponder::from_fn(move |err| {
            ack.respond(err);
            this.count.fetch_sub(1, Ordering::AcqRel);
            this.acked.notify_waiters();
        })
    }

    async fn drained(&self, ctx: &StreamContext) {
        let mut shutdown = ctx.shutdown_rx.clone();
        loop {
            let acked = self.acked.notified();
            if self.count.load(Ordering::Acquire) == 0 || ctx.is_shutting_down() {
                return;
            }
            tokio::select! {
                _ = acked => {}
                _ = shutdown.wait_for(|v| *v) => return,
            }
        }
    }
}

async fn feed(
    mut input: Box<dyn Input>,
    buffer: Arc<dyn Buffer>,
    ctx: StreamContext,
    read_err: Arc<Mutex<Option<String>>>,
) {
    let outstanding = Arc::new(Outstanding::default());
    while !ctx.is_shutting_down() {
        match input.read_batch(&ctx).await {
            Ok((batch, ack)) => {
                let ack = outstanding.track(ack);
                if let Err(e) = buffer.push(Transaction::new(batch, ack)).await {
                    tracing::error!("Failed to write to buffer: {e}");
                }
            }
            Err(CoreError::EndOfInput) => break,
            Err(CoreError::Timeout) => continue,
            Err(e) => {
                *read_err.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
                break;
            }
        }
    }
    outstanding.drained(&ctx).await;
    let _ = input.close().await;
    buffer.end_of_input();
}

#[async_trait]
impl Input for BufferedInput {
    fn input_type(&self) -> &'static str {
        self.input_type
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        if let Some(mut input) = self.inner.take() {
            input.connect(ctx).await?;
            let task = feed(
                input,
                self.buffer.clone(),
                ctx.clone(),
                self.read_err.clone(),
            );
            self.feeder = Some(tokio::spawn(task));
        }
        Ok(())
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        match self.buffer.pop(ctx).await {
            Ok(tx) => Ok((tx.payload, tx.ack)),
            Err(CoreError::EndOfInput) => {
                match self
                    .read_err
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                {
                    Some(e) => Err(CoreError::other(e)),
                    None => Err(CoreError::EndOfInput),
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        if let Some(feeder) = self.feeder.take() {
            feeder.abort();
        }
        self.buffer.end_of_input();
        self.buffer.close().await
    }
}

/// Bento's default `memory` buffer limit in bytes.
pub const DEFAULT_MEMORY_BUFFER_LIMIT: usize = 524_288_000;

/// The `memory` buffer (`internal/impl/pure/buffer_memory.go`): holds batches in memory up to
/// `limit` bytes and acknowledges each at the input once stored, as Bento does; a write waits
/// while the buffer is full, and a batch larger than the whole limit is nacked. The limit is
/// capped by the organization's `max_memory_bytes_per_run`. `buffer_backlog` is the bytes held.
/// ponytail: `batch_policy` is not read; batches leave as they arrived.
pub struct MemoryBuffer {
    limit: usize,
    state: Mutex<MemoryState>,
    items: tokio::sync::Notify,
    space: tokio::sync::Notify,
    ended: AtomicBool,
    backlog: Gauge,
}

#[derive(Default)]
struct MemoryState {
    queue: VecDeque<(Batch, usize)>,
    bytes: usize,
}

impl MemoryBuffer {
    pub fn new(limit: usize, backlog: Gauge) -> Self {
        Self {
            limit,
            state: Mutex::new(MemoryState::default()),
            items: tokio::sync::Notify::new(),
            space: tokio::sync::Notify::new(),
            ended: AtomicBool::new(false),
            backlog,
        }
    }

    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let limit = match conf.config.get("limit") {
            None | Some(serde_json::Value::Null) => DEFAULT_MEMORY_BUFFER_LIMIT,
            Some(v) => v
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| {
                    CoreError::config("buffer memory: limit must be a whole number of bytes")
                })?,
        };
        if limit == 0 {
            return Err(CoreError::config("buffer memory: limit must be above 0"));
        }
        let quota = ctx.org.get_quotas().max_memory_bytes_per_run;
        let backlog = ctx
            .metrics
            .component(&conf.label, "root.buffer")
            .map_or_else(Gauge::noop, |ns| ns.gauge("buffer_backlog"));
        Ok(Self::new(limit.min(quota), backlog))
    }

    fn lock(&self) -> MutexGuard<'_, MemoryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stores `payload` when it fits; answers whether it did.
    fn try_store(&self, payload: &mut Option<Batch>, size: usize) -> bool {
        let mut st = self.lock();
        if st.bytes + size > self.limit {
            return false;
        }
        if let Some(batch) = payload.take() {
            st.bytes += size;
            st.queue.push_back((batch, size));
            self.backlog.set(st.bytes as i64);
        }
        true
    }

    fn take(&self) -> Option<Batch> {
        let batch = {
            let mut st = self.lock();
            let (batch, size) = st.queue.pop_front()?;
            st.bytes -= size;
            self.backlog.set(st.bytes as i64);
            batch
        };
        self.space.notify_waiters();
        Some(batch)
    }

    /// Stores a transaction, waiting for room, and acknowledges it at the input.
    pub async fn write(&self, tx: Transaction) -> Result<(), CoreError> {
        let Transaction { payload, ack } = tx;
        let size: usize = payload.iter().map(Part::size_estimate).sum();
        if size > self.limit {
            let msg = format!(
                "batch of {size} bytes exceeds the memory buffer limit of {} bytes",
                self.limit
            );
            ack.nack(BatchError::new(payload, msg.clone()));
            return Err(CoreError::other(msg));
        }
        let mut payload = Some(payload);
        loop {
            let room = self.space.notified();
            if self.try_store(&mut payload, size) {
                break;
            }
            room.await;
        }
        ack.ack();
        self.items.notify_one();
        Ok(())
    }

    /// The oldest batch held; once the input has ended (or the stream stops) and nothing is
    /// left, `EndOfInput`.
    pub async fn read(
        &self,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<Transaction, CoreError> {
        loop {
            let ready = self.items.notified();
            if let Some(batch) = self.take() {
                return Ok(Transaction::new(batch, AckResponder::noop()));
            }
            if self.ended.load(Ordering::Acquire) || *shutdown.borrow() {
                return Err(CoreError::EndOfInput);
            }
            tokio::select! {
                _ = ready => {}
                _ = shutdown.wait_for(|v| *v) => return Err(CoreError::EndOfInput),
            }
        }
    }
}

#[async_trait]
impl Buffer for MemoryBuffer {
    async fn push(&self, tx: Transaction) -> Result<(), CoreError> {
        self.write(tx).await
    }

    async fn pop(&self, ctx: &StreamContext) -> Result<Transaction, CoreError> {
        self.read(ctx.shutdown_rx.clone()).await
    }

    fn end_of_input(&self) {
        self.ended.store(true, Ordering::Release);
        self.items.notify_waiters();
        self.space.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::prometheus::{Namespaced, PromExporter};

    type Calls = Arc<Mutex<Vec<bool>>>;

    fn tx(body: &'static str, calls: &Calls) -> Transaction {
        let calls = calls.clone();
        Transaction::new(
            vec![Part::from_str(body)],
            AckResponder::from_fn(move |e| calls.lock().unwrap().push(e.is_some())),
        )
    }

    fn body(t: &Transaction) -> String {
        String::from_utf8_lossy(&t.payload[0].as_bytes_ref()).into_owned()
    }

    #[tokio::test]
    async fn batches_leave_in_order_acked_at_once_with_the_backlog_in_bytes() {
        let gauge =
            Namespaced::bare(Arc::new(PromExporter::with_defaults())).gauge("buffer_backlog");
        let buf = MemoryBuffer::new(100, gauge.clone());
        let calls = Calls::default();
        buf.write(tx("ab", &calls)).await.unwrap();
        buf.write(tx("cde", &calls)).await.unwrap();
        assert_eq!(*calls.lock().unwrap(), [false, false]);
        assert_eq!(gauge.value(), 5.0);
        let (_stop, rx) = watch::channel(false);
        assert_eq!(body(&buf.read(rx.clone()).await.unwrap()), "ab");
        assert_eq!(gauge.value(), 3.0);
        buf.end_of_input();
        assert_eq!(body(&buf.read(rx.clone()).await.unwrap()), "cde");
        assert!(matches!(buf.read(rx).await, Err(CoreError::EndOfInput)));
        assert_eq!(gauge.value(), 0.0);
    }

    #[tokio::test]
    async fn a_batch_over_the_limit_is_nacked() {
        let buf = MemoryBuffer::new(2, Gauge::noop());
        let calls = Calls::default();
        assert!(buf.write(tx("abc", &calls)).await.is_err());
        assert_eq!(*calls.lock().unwrap(), [true]);
    }

    #[tokio::test]
    async fn a_full_buffer_holds_the_write_until_a_read() {
        let buf = Arc::new(MemoryBuffer::new(3, Gauge::noop()));
        let calls = Calls::default();
        buf.write(tx("ab", &calls)).await.unwrap();
        let (writer, more) = (buf.clone(), calls.clone());
        let pending = tokio::spawn(async move { writer.write(tx("cd", &more)).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!pending.is_finished());
        let (_stop, rx) = watch::channel(false);
        assert_eq!(body(&buf.read(rx.clone()).await.unwrap()), "ab");
        pending.await.unwrap().unwrap();
        assert_eq!(body(&buf.read(rx).await.unwrap()), "cd");
    }
}
