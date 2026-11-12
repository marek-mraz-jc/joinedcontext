//! The stream's buffer between its input and pipeline (`internal/component/buffer/stream.go`):
//! a task feeds every input transaction into the buffer, its ack passed through, and the pipeline
//! reads what the buffer releases. When the input ends the buffer is told (`end_of_input`).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::error::CoreError;
use crate::message::Batch;
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

async fn feed(
    mut input: Box<dyn Input>,
    buffer: Arc<dyn Buffer>,
    ctx: StreamContext,
    read_err: Arc<Mutex<Option<String>>>,
) {
    while !ctx.is_shutting_down() {
        match input.read_batch(&ctx).await {
            Ok((batch, ack)) => {
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
            let task = feed(input, self.buffer.clone(), ctx.clone(), self.read_err.clone());
            self.feeder = Some(tokio::spawn(task));
        }
        Ok(())
    }

    async fn read_batch(&mut self, ctx: &StreamContext) -> Result<(Batch, AckResponder), CoreError> {
        match self.buffer.pop(ctx).await {
            Ok(tx) => Ok((tx.payload, tx.ack)),
            Err(CoreError::EndOfInput) => {
                match self.read_err.lock().unwrap_or_else(|p| p.into_inner()).take() {
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
