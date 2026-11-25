//! One operator-supplied ANTARES_* value through every startup parser of the broker (metrics
//! port, sweep cadence, subject mesh identities, roles, the OTLP endpoint's log redaction): none
//! panics and each keeps its contract (T-7829).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 4096 {
        return;
    }
    if let Ok(text) = std::str::from_utf8(data) {
        antares_broker::fuzzing::config_value(text);
    }
});
