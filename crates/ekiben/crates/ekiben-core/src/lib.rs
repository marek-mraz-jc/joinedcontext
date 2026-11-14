#![forbid(unsafe_code)]
//! `ekiben-core` — Core message, batch, transaction, pipeline, resource,
//! configuration interpolation, and multi-organization quota engine for `ekiben`.

pub mod buffer;
pub mod config;
pub mod error;
pub mod interpolate;
pub mod message;
pub mod metrics;
pub mod observed;
pub mod org;
pub mod paging;
pub mod pipeline;
pub mod prometheus;
pub mod redact;
pub mod registry;
pub mod token_backoff;
pub mod traits;
pub mod transaction;

pub use config::{
    BackoffConfig, BatchPolicyConfig, BufferConfig, ComponentConfig, EnvAccess, InputConfig,
    OverlapPolicy, PipelineConfig, ProcessorConfig, ResourceConfigs, RunMode, ScheduleConfig,
    StreamConfig, ENV_VAR_ALLOW_PREFIX,
};
pub use error::{BatchError, CoreError, DetailedError};
pub use interpolate::{interpolate_env_bytes, interpolate_env_str, EnvLintError};
pub use message::{
    deserialize_bytes, serialize_bytes, Batch, Part, PartMetaAdapter, PartsView, SortGroup,
};
pub use metrics::{MetricsRegistry, StreamMetrics, TimingStats};
pub use observed::{ObservedInput, ObservedOutput};
pub use org::{HostTarget, OrgContext, OrgId, OrgQuotas, ResourceManager};
pub use paging::{execute_paged, Paging, DEFAULT_PAGE_ROWS};
pub use pipeline::{
    execute_all, execute_catch_all, execute_try_all, Batcher, ObservedProcessor, PipelineWorkerPool,
};
pub use prometheus::{
    Counter, Gauge, MetricMapping, MetricsConfig, Namespaced, PromExporter, SeriesOwner, Timer,
};
pub use redact::{redact_config_value, redact_url_str, SecretString};
pub use registry::{lint_fields, LintFn, LintReport, Registry};
pub use token_backoff::TokenBackoff;
pub use traits::{
    Buffer, Cache, Input, Output, Pages, ProcContext, Processor, RateLimit, RunSummary,
    StreamContext,
};
pub use transaction::{
    AckResponder, CombinedAcker, SyncResponseStore, TrackedTransaction, Transaction,
};
