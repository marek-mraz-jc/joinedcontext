#![forbid(unsafe_code)]
//! `ekiben-io-http` — Outbound HTTP client (`rustls`, SSRF guard, OAuth2, retries, size caps, secret redaction),
//! `http` processor, `http_client` input, and `http_client` output.

pub mod client;
pub mod input;
pub mod output;
pub mod processor;

use ekiben_core::Registry;

pub fn register_all(registry: &Registry) {
    processor::register(registry);
    input::register(registry);
    output::register(registry);
}
