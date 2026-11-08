#![forbid(unsafe_code)]
//! `ekiben-io-ngsild` — the NGSI-LD connector pair (ekiben extension, beside Bento compatibility):
//! the `ngsi_ld` processor and output that write entity batches through a gateway Endpoint (E5),
//! and the `ngsi_ld` input that reads them by query, temporal query or Notification (E6).
//!
//! Every outbound call goes through [`outbound`] (public addresses only, or the organization's
//! `private_hosts`), every token through [`token`] (client credentials, cached per organization,
//! never logged). See README.md for the fields and the platform contract.

pub mod client;
pub mod config;
pub mod input;
pub mod metrics;
pub mod notification;
pub mod outbound;
pub mod output;
pub mod processor;
pub mod token;
pub mod writer;

use ekiben_core::Registry;

/// The component names this crate registers, each an ekiben extension Bento refuses.
pub const EXTENSION_COMPONENTS: &[(&str, &str)] = &[
    ("inputs", "ngsi_ld"),
    ("outputs", "ngsi_ld"),
    ("processors", "ngsi_ld"),
];

pub fn register_all(registry: &Registry) {
    processor::register(registry);
    output::register(registry);
    input::register(registry);
}
