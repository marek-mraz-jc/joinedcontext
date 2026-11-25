//! The keyset `cursor` query parameter: never panics, and a cursor the
//! broker signed reads back as the id it carries.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // a query string is capped far below this
    if data.len() > 4096 {
        return;
    }
    let _ = antares_api::fuzzing::paging_cursor(data);
});
