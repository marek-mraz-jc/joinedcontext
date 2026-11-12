#![forbid(unsafe_code)]
//! `ekiben-text` — Bento 1.22.0's text and document processors without network access: `jq`,
//! `jmespath`, `grok`, `msgpack` and `parse_log`, one module per Go file.

mod fields;
pub mod go_regex;
pub mod go_syslog;
mod go_unicode_names;
mod grok_patterns;
pub mod msgpack_processor;
pub mod processor_grok;
pub mod processor_jmespath;
pub mod processor_jq;
pub mod processor_parse_log;

use ekiben_core::Registry;

pub fn register(registry: &Registry) {
    processor_jq::register(registry);
    processor_jmespath::register(registry);
    processor_grok::register(registry);
    msgpack_processor::register(registry);
    processor_parse_log::register(registry);
}
