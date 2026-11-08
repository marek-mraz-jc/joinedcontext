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
pub use manager::{StreamEntryStatus, StreamManager};
pub use metrics_config::load_metrics_config;
pub use org_vars::OrgVarsDir;
pub use runner::{run_stream_once, RunningStreamHandle};
pub use scheduler::{ScheduledStreamEntry, Scheduler};
pub use tokens::OrgTokenFile;
