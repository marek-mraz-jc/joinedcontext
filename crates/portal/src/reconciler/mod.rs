//! The embedded reconciler (T-0191, CC-03, CC-54, CC-55, MF-11).
//!
//! The Portal carries `jcctl` as a library rather than shelling out to it: the same loader
//! that validates a repository on the command line validates it here, so the Portal cannot
//! disagree with the CLI about what a manifest is.
//!
//! One replica reconciles. [`leader`] elects it with a PostgreSQL advisory lock, and
//! [`daemon`] is the loop that reads the repository and refreshes the live-state mirror.

pub mod agreements;
pub mod app_clients;
pub mod app_hosts;
pub mod catalog;
pub mod ckan;
pub mod ckan_dashboards;
pub mod daemon;
pub mod drift;
pub mod edge_file;
pub mod foreign;
pub mod groups;
pub mod hub_scopes;
pub mod leader;
pub mod mcp_clients;
pub mod mqtt_brokers;
pub mod org_hosts;
pub mod pipeline_authors;
pub mod realm;
pub mod registrations;
pub mod removal_hold;
pub mod representations;
pub mod run_queue;
pub mod space_retention;
pub mod space_rules;
pub mod staging;
pub mod stall;
pub mod streams;
pub mod subscriptions;
pub mod temporal_q;
pub mod transitions;
pub mod views;
pub mod warned;
pub mod wasm_shards;
pub mod workload_clients;

pub use daemon::{SyncError, SyncStatus, Syncer};
pub use leader::Leadership;
pub use streams::{StreamDeployer, StreamOutcome};

/// What `run` logs, for the tests that assert a wave's warnings: a scoped subscriber also makes
/// tracing evaluate the event's fields, which no subscriber at all skips.
#[cfg(test)]
pub(crate) async fn logged<F: std::future::Future>(run: F) -> (F::Output, String) {
    #[derive(Clone, Default)]
    struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let sink = Sink::default();
    let writer = sink.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    let out = run.await;
    drop(guard);
    let text =
        String::from_utf8_lossy(&sink.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
    (out, text)
}
