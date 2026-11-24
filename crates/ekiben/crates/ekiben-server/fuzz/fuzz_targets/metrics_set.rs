//! `--set metrics.<path>=<value>` arguments, applied in order onto one tree (one per line), then
//! read as the metrics config.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut root = serde_json::json!({});
    for arg in text.lines().take(64) {
        let _ = ekiben_server::metrics_config::apply_set(&mut root, arg);
    }
    let section = root.get("metrics").cloned().unwrap_or_default();
    let _ = ekiben_core::MetricsConfig::from_json(&section);
});
