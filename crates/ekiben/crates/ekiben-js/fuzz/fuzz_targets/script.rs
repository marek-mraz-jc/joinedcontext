//! Input: a script, a NUL byte, then the message. Without a NUL the whole input is the script
//! and the message is `{"a":1}`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (code, message) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &br#"{"a":1}"#[..]),
    };
    let code = String::from_utf8_lossy(code);
    let flags = ekiben_js::fuzzing::run(&code, message);
    assert!(flags.len() <= 2);
});
