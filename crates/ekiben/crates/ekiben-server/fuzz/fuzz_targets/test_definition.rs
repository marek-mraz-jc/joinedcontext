//! The `tests:` list of `ekiben test`: any YAML gives cases or an error.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = ekiben_server::unit_test::parse_definition(text);
    }
});
