#![forbid(unsafe_code)]
//! `ekiben-io-nats` — NATS Core, NATS JetStream, and NATS Key-Value inputs, outputs,
//! processors, and cache resources, following `internal/impl/nats/*.go`.

pub mod broker_mem;
pub mod cache_kv;
pub mod connection;
pub mod input_core;
pub mod input_jetstream;
pub mod kv_backend;
pub mod output_core;
pub mod output_jetstream;
pub mod processor_kv;

use ekiben_core::Registry;

pub fn register_all(registry: &Registry) {
    input_core::register(registry);
    output_core::register(registry);
    input_jetstream::register(registry);
    output_jetstream::register(registry);
    cache_kv::register(registry);
    processor_kv::register(registry);
}
