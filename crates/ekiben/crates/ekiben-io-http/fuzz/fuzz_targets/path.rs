#![no_main]
//! A route pattern (first line) and a request path: a match names at most one value per segment.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let (pattern, path) = text.split_once('\n').unwrap_or((&text, "/"));
    if let Some(vars) = ekiben_io_http::fuzzing::path(pattern, path) {
        assert!(vars <= pattern.split('/').count());
    }
});
