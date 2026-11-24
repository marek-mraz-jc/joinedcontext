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
pub mod paginate;
pub mod processor;
#[cfg(feature = "websocket")]
pub mod redis_cache;
pub mod socket;
#[cfg(feature = "websocket")]
pub mod websocket;

use ekiben_core::Registry;

/// The parsers that read bytes from outside (MQTT wire, multipart bodies, media types, paths), for
/// fuzz/ (cargo fuzz): each must answer or refuse, never panic or allocate past its input.
#[cfg(feature = "fuzzing")]
pub mod fuzzing {
    /// Splits MQTT packets off `buf` and reads each PUBLISH; how many packets were whole.
    #[cfg(feature = "websocket")]
    pub fn mqtt_wire(mut buf: Vec<u8>) -> usize {
        crate::mqtt::fuzz_wire(&mut buf)
    }

    /// Reads a Content-Type and, when it names a boundary, the multipart body under it.
    pub fn multipart(content_type: &str, body: &[u8]) -> Option<usize> {
        crate::http_server::fuzz_multipart(content_type, body)
    }

    /// Decodes a request path and matches it against a route pattern read from `pattern`.
    pub fn path(pattern: &str, path: &str) -> Option<usize> {
        crate::http_server::fuzz_path(pattern, path)
    }
}

pub fn register_all(registry: &Registry) {
    processor::register(registry);
    input::register(registry);
    output::register(registry);
    http_server::register(registry);
    socket::register(registry);
    #[cfg(feature = "websocket")]
    websocket::register(registry);
    #[cfg(feature = "websocket")]
    mqtt::register(registry);
    #[cfg(feature = "websocket")]
    redis_cache::register(registry);
}
