//! Decodes MessagePack; a decoded value must encode and decode back to itself.
#![no_main]

use ekiben_bloblang::msgpack::{decode, encode};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(value) = decode(data) else {
        return;
    };
    let again = decode(&encode(&value)).expect("own encoding decodes");
    assert_eq!(encode(&again), encode(&value));
});
