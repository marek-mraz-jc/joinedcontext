#![forbid(unsafe_code)]
//! `ekiben-core` — Core message, batch, transaction, pipeline, resource,
//! configuration interpolation, and multi-organization quota engine for `ekiben`.

pub mod config;
pub mod error;
pub mod interpolate;
pub mod message;
pub mod metrics;
pub mod org;
pub mod pipeline;
pub mod redact;
pub mod registry;
pub mod traits;
pub mod transaction;

pub use config::{
    BackoffConfig, BatchPolicyConfig, BufferConfig, ComponentConfig, InputConfig, OverlapPolicy,
    PipelineConfig, ProcessorConfig, ResourceConfigs, RunMode, ScheduleConfig, StreamConfig,
};
pub use error::{BatchError, CoreError, DetailedError};
pub use interpolate::{interpolate_env_bytes, interpolate_env_str, EnvLintError};
pub use message::{
    deserialize_bytes, serialize_bytes, Batch, Part, PartMetaAdapter, PartsView, SortGroup,
};
pub use metrics::{MetricsRegistry, StreamMetrics, TimingStats};
pub use org::{OrgContext, OrgId, OrgQuotas, ResourceManager};
pub use pipeline::{
    execute_all, execute_catch_all, execute_try_all, Batcher, ObservedProcessor, PipelineWorkerPool,
};
pub use redact::{redact_config_value, redact_url_str, SecretString};
pub use registry::{LintReport, Registry};
pub use traits::{
    Buffer, Cache, Input, Output, ProcContext, Processor, RateLimit, RunSummary, StreamContext,
};
pub use transaction::{
    AckResponder, CombinedAcker, SyncResponseStore, TrackedTransaction, Transaction,
};
