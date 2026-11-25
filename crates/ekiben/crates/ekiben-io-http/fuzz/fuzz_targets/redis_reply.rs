#![no_main]
//! Any bytes a redis server sends: whole replies are read off the front, the rest refused.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let whole = ekiben_io_http::fuzzing::redis_replies(data);
    // Every reply takes at least its type byte, so the reader always ends.
    assert!(whole <= data.len());
});
