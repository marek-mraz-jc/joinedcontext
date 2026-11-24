//! Input: a component config as JSON, linted as every sql component lints it.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(conf) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    ekiben_io_sql::fuzzing::lint_all(&conf);
});
