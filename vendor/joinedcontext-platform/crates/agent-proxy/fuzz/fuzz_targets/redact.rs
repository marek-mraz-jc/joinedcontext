//! Diagnostics text and event payloads, any bytes: a redacted text cut to its cap fits it on a
//! character boundary, and a payload with nothing to redact comes back as it went in (T-7795).
#![no_main]

use agent_proxy::routes::{diagnostics, events};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let [cap, text @ ..] = data else {
        return;
    };
    let Ok(text) = std::str::from_utf8(text) else {
        return;
    };
    diagnostics::fuzzing::text(text, u64::from(*cap) * 4);
    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(text) {
        events::fuzzing::payload(&payload);
    }
});
