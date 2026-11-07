//! What the reconciler drives on a schedule, and the two hops `jcctl` deliberately does not
//! have: a socket to the outside and a write path into the forge.
//!
//! `jcctl` decides; this decides nothing. The foreign-model mirror (DM-48, DM-49) and the
//! `SyncSource` loop (MF-27…MF-32) both end in [`proposal`], so there is one place where a
//! schedule turns into a merge request.

pub mod driver;
pub(crate) mod guard;
pub mod mirror;
pub mod proposal;
pub mod remote;
pub mod schema_api;
pub mod state;
pub mod webhook_secrets;
