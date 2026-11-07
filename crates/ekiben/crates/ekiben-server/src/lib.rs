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
pub use cron::CronSchedule;
pub use manager::{StreamEntryStatus, StreamManager};
pub use runner::{run_stream_once, RunningStreamHandle};
pub use scheduler::{ScheduledStreamEntry, Scheduler};
