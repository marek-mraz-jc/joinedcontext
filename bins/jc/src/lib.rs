#![forbid(unsafe_code)]
//! `jc`: the joinedcontext modules in one process.
//!
//! One tokio runtime, one tracing subscriber, one metrics recorder, one allocator, one rustls
//! crypto provider and one shutdown token; every module keeps its own state and auth and is
//! reached only through its mount in `jc-router` (one domain with a slug per module, a host per
//! module, or both; overlapping mounts are refused at start).

pub mod app;
pub mod config;
pub mod door;
pub mod module;
mod modules;
pub mod pg;
mod provision;
pub mod runtime;
pub mod setup;
pub mod siblings;

pub use app::{MountPlan, Running, StartError, build_router, plan_mounts, run_until, start};
pub use config::{Cli, Config, MountMode};
pub use module::Module;
