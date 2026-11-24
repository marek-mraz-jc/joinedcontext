//! The gateway's metrics page the quality checks read: any text yields samples or none.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = joinedcontext_portal::quality_checks::parse(text);
    }
});
