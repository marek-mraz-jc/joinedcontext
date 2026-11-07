#![forbid(unsafe_code)]
//! `ekiben-io-file` — Local and utility inputs/outputs:
//! `file`, `stdout`, `generate`, `broker`, `resource`, `drop`, `drop_on`, `switch`, `fallback`, `retry`, `reject`, `reject_errored`, `cache`, `sync_response`.

pub mod inputs;
pub mod outputs;
pub mod path_guard;

use ekiben_core::Registry;

pub fn register_all(registry: &Registry) {
    inputs::register(registry);
    outputs::register(registry);
}
