//! Any JSON write body: stripping the gateway-owned attributes never panics, and what it
//! leaves names none of them (T-7767).
#![no_main]

use context_gateway::quality;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(mut payload) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    quality::strip_owned(&mut payload);
    let remaining: Vec<String> = match &payload {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_object())
            .flat_map(|map| map.keys().cloned())
            .collect(),
        _ => Vec::new(),
    };
    for name in remaining {
        assert!(!quality::is_owned(&name), "{name} survived");
    }
    if let Some(text) = payload.as_str() {
        assert_eq!(quality::owned_path(Some(text)).is_some(), quality::is_owned(text));
    }
});
