#![no_main]
//! A Content-Type (first line) and a body: the multipart parts come back or the body is refused.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let (content_type, body) = text.split_once('\n').unwrap_or((&text, ""));
    if let Some(parts) = ekiben_io_http::fuzzing::multipart(content_type, body.as_bytes()) {
        assert!(parts <= body.len());
    }
});
