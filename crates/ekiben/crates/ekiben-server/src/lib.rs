#![forbid(unsafe_code)]
//! `ekiben-server` — Streams-mode HTTP API server (`axum`), cron/period scheduler,
//! one-shot stream runner, config linter, and CLI entrypoints.

pub mod api;
pub mod blobl;
pub mod cron;
pub mod manager;
pub mod runner;
pub mod scheduler;
pub mod unit_test;

pub use api::{build_router, ServerAuthConfig, ServerState};

use std::sync::Arc;

use ekiben_core::Registry;

/// The registry of every component this build carries (processors, HTTP, file, NATS, and with
/// their features Parquet and Protobuf), as the `ekiben` binary and a hosting `jc` use it.
pub fn build_full_registry() -> Arc<Registry> {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    ekiben_io_http::register_all(&reg);
    ekiben_io_file::register_all(&reg);
    ekiben_io_nats::register_all(&reg);
    #[cfg(feature = "parquet")]
    ekiben_parquet::register(&reg);
    #[cfg(feature = "protobuf")]
    ekiben_protobuf::register(&reg);
    reg
}
pub use cron::CronSchedule;
pub use manager::{StreamEntryStatus, StreamManager};
pub use runner::{run_stream_once, RunningStreamHandle};
pub use scheduler::{ScheduledStreamEntry, Scheduler};
