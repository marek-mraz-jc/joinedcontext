#![no_main]
//! Any text in an address, path, from or mailbox field: read safely or refused.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    ekiben_io_remote::fuzzing::names(&String::from_utf8_lossy(data));
});
