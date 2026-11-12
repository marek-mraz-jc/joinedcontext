#![forbid(unsafe_code)]
//! `ekiben-processors` — Built-in Bento processors, local rate-limit, and memory cache resources.

pub mod archive;
pub mod branch;
pub mod buffer_system_window;
pub mod flow;
pub mod json_schema;
pub mod mapping;
pub mod processor_cached;
pub mod processor_group_by;
pub mod processor_group_by_value;
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
    json_schema::register(registry);
    archive::register(registry);
    utility::register(registry);
    resources::register(registry);
    registry.register_processor("group_by", |conf, ctx| {
        Ok(std::sync::Arc::new(processor_group_by::GroupByProcessor::from_config(conf, ctx)?))
    });
    registry.register_processor("group_by_value", |conf, ctx| {
        Ok(std::sync::Arc::new(
            processor_group_by_value::GroupByValueProcessor::from_config(conf, ctx)?,
        ))
    });
    registry.register_buffer("system_window", |conf, ctx| {
        Ok(std::sync::Arc::new(
            buffer_system_window::SystemWindowBuffer::from_config(conf, ctx)?,
        ))
    });
    registry.register_processor("cached", |conf, ctx| {
        Ok(std::sync::Arc::new(processor_cached::CachedProcessor::from_config(conf, ctx)?))
    });
}
