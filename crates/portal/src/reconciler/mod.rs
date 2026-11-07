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
pub mod daemon;
pub mod drift;
pub mod edge_file;
pub mod foreign;
pub mod groups;
pub mod hub_scopes;
pub mod leader;
pub mod mcp_clients;
pub mod mqtt_brokers;
pub mod pipeline_authors;
pub mod project_teams;
pub mod realm;
pub mod registrations;
pub mod representations;
pub mod run_queue;
pub mod space_rules;
pub mod staging;
pub mod stall;
pub mod streams;
pub mod subscriptions;
pub mod temporal_q;
pub mod transitions;
pub mod views;
pub mod wasm_shards;
pub mod workload_clients;

pub use daemon::{SyncError, SyncStatus, Syncer};
pub use leader::Leadership;
pub use streams::{StreamDeployer, StreamOutcome};
