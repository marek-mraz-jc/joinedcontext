//! A subscription-mirror delta read off the NATS KV bucket (`decode_delta`): any bytes never
//! panic, and a row it accepts names a tenant inside the mirror's grammar (T-7829).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    antares_broker::fuzzing::mirror_delta(data);
});
