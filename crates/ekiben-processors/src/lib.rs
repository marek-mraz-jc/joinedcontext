#![forbid(unsafe_code)]
//! `ekiben-processors` — Built-in Bento processors, local rate-limit, and memory cache resources.

pub mod archive;
pub mod branch;
pub mod flow;
pub mod mapping;
pub mod resources;
pub mod switch;
pub mod utility;
pub mod workflow;

use ekiben_core::Registry;

/// Registers all standard processors, `local` rate_limit, and `memory`/`lru`/`ttlru`/`noop` caches into `registry`.
pub fn register_all(registry: &Registry) {
    mapping::register(registry);
    branch::register(registry);
    workflow::register(registry);
    switch::register(registry);
    flow::register(registry);
    archive::register(registry);
    utility::register(registry);
    resources::register(registry);
}
