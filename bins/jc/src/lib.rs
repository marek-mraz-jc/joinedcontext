#![forbid(unsafe_code)]
//! `jc`: the joinedcontext modules in one process.
//!
//! One tokio runtime, one tracing subscriber, one metrics recorder, one allocator, one rustls
//! crypto provider and one shutdown token; every module keeps its own state and auth and is
//! reached only through its mount in `jc-router` (one domain with a slug per module, a host per
//! module, or both; overlapping mounts are refused at start).

pub mod access;
pub mod app;
pub mod config;
pub mod doctor;
pub mod door;
pub mod module;
mod modules;
pub mod pg;
mod provision;
pub mod runtime;
pub mod setup;
pub mod siblings;
mod slow;
#[cfg(feature = "kaisatsu")]
pub mod split_realm;

/// The lines `f` logs through a plain fmt subscriber (no colour), for tests of what `jc` writes.
/// The lines go to an unnamed temporary file (`&File` writes, so `Arc<File>` is a writer).
#[cfg(test)]
pub(crate) fn captured(f: impl FnOnce()) -> String {
    use std::io::{Read as _, Seek as _};
    let file = std::sync::Arc::new(tempfile::tempfile().expect("a temporary log file"));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(std::sync::Arc::clone(&file))
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let mut log = String::new();
    let mut reader = &*file;
    reader.rewind().expect("rewind the log");
    reader.read_to_string(&mut log).expect("utf8 log");
    log
}

pub use app::{MountPlan, Running, StartError, build_router, plan_mounts, run_until, start};
pub use config::{Cli, Config, MountMode};
pub use module::Module;
/// Model Tools' `/generate` on a parsed body, the entry of its fuzz target (bins/jc/fuzz).
#[cfg(feature = "model-tools")]
#[doc(hidden)]
pub use modules::model_tools::generate as model_tools_generate;
/// The slow-statement fingerprint, the entry of its fuzz target (bins/jc/fuzz).
#[doc(hidden)]
pub use slow::fingerprint as sql_fingerprint;
