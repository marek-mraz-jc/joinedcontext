//! 5.2.23 Query bodies, plain and temporal: never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // the body bound refuses far larger documents before the parser sees one
    if data.len() > 16384 {
        return;
    }
    let _ = antares_api::fuzzing::query_document(data);
});
