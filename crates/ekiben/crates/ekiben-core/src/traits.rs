use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_bloblang::Environment;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::error::{CoreError, DetailedError};
use crate::message::{Batch, Part};
use crate::metrics::StreamMetrics;
use crate::org::{OrgContext, OrgId, ResourceManager};
use crate::registry::Registry;
use crate::transaction::{AckResponder, Transaction};

/// Summary of a finished stream run (`ekiben run` JSON line & `GET /streams` `last_run` field).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub stream_id: String,
    pub org_id: String,
    pub messages_in: u64,
    pub messages_out: u64,
    pub messages_failed: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub duration_ms: u64,
    pub peak_rss_bytes: Option<u64>,
    pub success: bool,
    pub error: Option<String>,
}

/// Execution context passed to processors (`internal/component/processor/auto_observed.go:BatchProcContext`).
#[derive(Clone)]
pub struct ProcContext {
    pub stream_id: String,
    pub org: Arc<OrgContext>,
    pub resources: ResourceManager,
    pub registry: Arc<Registry>,
    pub blobl_env: Environment,
    pub metrics: Arc<StreamMetrics>,
    pub label: String,
    pub component_type: String,
    pub path: String,
    pub file_root: Option<std::path::PathBuf>,
    pub shutdown_rx: watch::Receiver<bool>,
}

impl ProcContext {
    pub fn org_id(&self) -> &OrgId {
        &self.org.org_id
    }

    pub fn into_child(
        &self,
        component_type: impl Into<String>,
        label: impl Into<String>,
        path_seg: impl AsRef<str>,
    ) -> Self {
        let mut next = self.clone();
        next.component_type = component_type.into();
        let lbl = label.into();
        if !lbl.is_empty() {
            next.label = lbl;
        }
        if next.path.is_empty() {
            next.path = path_seg.as_ref().to_string();
        } else {
            next.path = format!("{}.{}", next.path, path_seg.as_ref());
        }
        next
    }

    /// Marks `part` with a `DetailedError` carrying this processor's type, label, and config path,
    /// and increments the processor's error metric counter (`BatchProcContext.OnError`).
    pub fn on_error(&self, part: &mut Part, err: impl Into<String>) {
        let msg = err.into();
        self.metrics
            .incr_processor_error_at(&self.label, &self.path, 1);
        let detailed = DetailedError::new(
            msg,
            self.component_type.clone(),
            self.label.clone(),
            self.path.clone(),
        );
        part.error_set(Some(detailed));
    }

    pub fn is_shutting_down(&self) -> bool {
        *self.shutdown_rx.borrow()
    }
}

/// Execution context passed to inputs and outputs.
#[derive(Clone)]
pub struct StreamContext {
    pub stream_id: String,
    pub org: Arc<OrgContext>,
    pub resources: ResourceManager,
    pub registry: Arc<Registry>,
    pub blobl_env: Environment,
    pub metrics: Arc<StreamMetrics>,
    pub label: String,
    pub path: String,
    pub file_root: Option<std::path::PathBuf>,
    pub env_vars: Arc<HashMap<String, String>>,
    pub shutdown_rx: watch::Receiver<bool>,
}

impl StreamContext {
    pub fn org_id(&self) -> &OrgId {
        &self.org.org_id
    }

    pub fn to_proc_context(
        &self,
        component_type: impl Into<String>,
        label: impl Into<String>,
        path: impl Into<String>,
    ) -> ProcContext {
        ProcContext {
            stream_id: self.stream_id.clone(),
            org: self.org.clone(),
            resources: self.resources.clone(),
            registry: self.registry.clone(),
            blobl_env: self.blobl_env.clone(),
            metrics: self.metrics.clone(),
            label: label.into(),
            component_type: component_type.into(),
            path: path.into(),
            file_root: self.file_root.clone(),
            shutdown_rx: self.shutdown_rx.clone(),
        }
    }

    pub fn is_shutting_down(&self) -> bool {
        *self.shutdown_rx.borrow()
    }
}

#[async_trait]
pub trait Processor: Send + Sync {
    fn processor_type(&self) -> &'static str;
    fn label(&self) -> &str {
        ""
    }
    async fn process_batch(&self, ctx: &ProcContext, batch: Batch)
        -> Result<Vec<Batch>, CoreError>;
    /// Whether `process_batch` can fail the whole batch (an `Err` other than a quota or a
    /// close), so its caller keeps a copy of the input to flag. `false` hands the batch over
    /// whole, which lets the processor free each input part once its output exists.
    fn may_fail_batch(&self) -> bool {
        true
    }
    /// What `process_batch` would return, as pages of at most `rows` messages made one at a time,
    /// for a processor that expands a message into many (`unarchive`): only the page running
    /// exists. `Err` hands the batch back to run through `process_batch` (the default).
    fn pages(&self, _ctx: &ProcContext, batch: Batch, _rows: usize) -> Result<Box<Pages>, Batch> {
        Err(batch)
    }
    async fn close(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

/// The pages of [`Processor::pages`], each made when it is asked for.
pub type Pages = dyn Iterator<Item = Batch> + Send;

#[async_trait]
pub trait Input: Send + Sync {
    fn input_type(&self) -> &'static str;
    fn label(&self) -> &str {
        ""
    }
    async fn connect(&mut self, _ctx: &StreamContext) -> Result<(), CoreError> {
        Ok(())
    }
    async fn read_batch(&mut self, ctx: &StreamContext)
        -> Result<(Batch, AckResponder), CoreError>;
    fn is_connected(&self) -> bool {
        true
    }
    async fn close(&mut self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
pub trait Output: Send + Sync {
    fn output_type(&self) -> &'static str;
    fn label(&self) -> &str {
        ""
    }
    async fn connect(&self, _ctx: &StreamContext) -> Result<(), CoreError> {
        Ok(())
    }
    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError>;
    fn is_connected(&self) -> bool {
        true
    }
    async fn close(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
pub trait Cache: Send + Sync {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError>;
    async fn set(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError>;
    async fn add(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError>;
    async fn delete(&self, key: &str) -> Result<(), CoreError>;
    async fn exists(&self, key: &str) -> Result<bool, CoreError> {
        match self.get(key).await {
            Ok(_) => Ok(true),
            Err(CoreError::KeyNotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
    async fn close(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
pub trait RateLimit: Send + Sync {
    async fn access(&self, parts: &[Part]) -> Result<Duration, CoreError>;
    async fn close(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
pub trait Buffer: Send + Sync {
    async fn push(&self, tx: Transaction) -> Result<(), CoreError>;
    async fn pop(&self, ctx: &StreamContext) -> Result<Transaction, CoreError>;
    /// The input has ended: a buffer drains or releases what it holds (Bento's `EndOfInput`).
    fn end_of_input(&self) {}
    async fn close(&self) -> Result<(), CoreError> {
        Ok(())
    }
}
