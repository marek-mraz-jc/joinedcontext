#![no_main]
//! Any bytes a broker sends: whole packets are split off and each PUBLISH read, the rest refused.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let whole = ekiben_io_http::fuzzing::mqtt_wire(data.to_vec());
    // Every packet takes at least its fixed header's two bytes.
    assert!(whole <= data.len() / 2);
});
