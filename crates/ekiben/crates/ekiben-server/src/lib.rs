#![forbid(unsafe_code)]
//! `ekiben-server` — Streams-mode HTTP API server (`axum`), cron/period scheduler,
//! one-shot stream runner, config linter, and CLI entrypoints.

pub mod api;
pub mod blobl;
pub mod config_dir;
pub mod cron;
pub mod manager;
pub mod metrics_config;
pub mod org_vars;
pub mod runner;
pub mod scheduler;
pub mod tokens;
pub mod unit_test;

pub use api::{build_router, Caller, ServerAuthConfig, ServerState};
pub use cron::CronSchedule;
pub use manager::{StreamEntryStatus, StreamManager, StreamPersistence};
pub use metrics_config::load_metrics_config;
pub use org_vars::OrgVarsDir;
pub use runner::{run_stream_once, RunningStreamHandle};
pub use scheduler::{ScheduledStreamEntry, Scheduler};
pub use tokens::OrgTokenFile;

use std::sync::Arc;

use ekiben_core::Registry;

/// The registry of every component this build carries (processors, HTTP, file, NATS, and with
/// their features NGSI-LD, Parquet and Protobuf), as the `ekiben` binary and a hosting `jc` use it.
pub fn build_full_registry() -> Arc<Registry> {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    ekiben_io_http::register_all(&reg);
    ekiben_io_file::register_all(&reg);
    ekiben_io_nats::register_all(&reg);
    #[cfg(feature = "ngsild")]
    ekiben_io_ngsild::register_all(&reg);
    #[cfg(feature = "parquet")]
    ekiben_parquet::register(&reg);
    #[cfg(feature = "protobuf")]
    ekiben_protobuf::register(&reg);
    reg
}
