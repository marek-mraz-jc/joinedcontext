#![forbid(unsafe_code)]
//! `ekiben-io-file` — Local and utility inputs/outputs:
//! `file`, `stdout`, `generate`, `broker`, `resource`, `drop`, `drop_on`, `switch`, `fallback`, `retry`, `reject`, `reject_errored`, `cache`, `sync_response`.

pub mod inputs;
pub mod outputs;
pub mod path_guard;
pub mod scanner_decompress;
pub mod scanner_re_match;
pub mod scanner_skip_bom;
pub mod scanner_xml;

use ekiben_core::Registry;

pub fn register_all(registry: &Registry) {
    inputs::register(registry);
    outputs::register(registry);
}
