//! Any text as a LinkML model: the personal-data marks and the query attributes it names are read
//! without a panic (T-7766).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = jc_core::personal::marks(text);
    let _ = jc_core::display_order::query_attributes(text);
});
