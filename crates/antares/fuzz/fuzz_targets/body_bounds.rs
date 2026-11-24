//! The body depth scan and the deployment knobs' parsers: never panic, and
//! what they answer stays inside its bounds.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // the body cap (ANTARES_MAX_BODY_BYTES) is far above this; the scan is
    // linear, so a longer input finds nothing a short one cannot
    if data.len() > 65_536 {
        return;
    }
    let _ = antares_api::fuzzing::body_bounds(data);
});
