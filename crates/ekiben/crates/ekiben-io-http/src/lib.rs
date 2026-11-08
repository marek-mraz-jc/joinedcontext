#![forbid(unsafe_code)]
//! `ekiben-io-http` — Outbound HTTP client (`rustls`, SSRF guard, OAuth2, retries, size caps, secret redaction),
//! `http` processor, `http_client` input, `http_client` output, `http_server` input with the
//! shared ingest listener, and the `websocket` input (feature `websocket`, on by default).

pub mod client;
pub mod http_server;
pub mod ingest;
pub mod input;
#[cfg(feature = "websocket")]
pub mod mqtt;
pub mod outbound;
pub mod output;
pub mod processor;
#[cfg(feature = "websocket")]
pub mod websocket;

use ekiben_core::Registry;

pub fn register_all(registry: &Registry) {
    processor::register(registry);
    input::register(registry);
    output::register(registry);
    http_server::register(registry);
    #[cfg(feature = "websocket")]
    websocket::register(registry);
    #[cfg(feature = "websocket")]
    mqtt::register(registry);
}
