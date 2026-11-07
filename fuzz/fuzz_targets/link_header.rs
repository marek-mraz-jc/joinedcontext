//! The `Link` header naming a request's @context (TS 104 176 6.2.4); one
//! header per input line: never panics (T-3463).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // the API caps a query string and a header far below this, so no
    // allocation the target sees scales past what a request can carry
    if data.len() > 4096 {
        return;
    }
    let _ = antares_api::fuzzing::link_header(data);
});
