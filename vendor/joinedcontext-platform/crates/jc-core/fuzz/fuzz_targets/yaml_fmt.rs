//! Any text through the manifest formatter: it answers and never panics, and what it writes it
//! formats again (T-7766).
#![no_main]

use jc_core::yaml_fmt::format;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(out) = format(text) {
        assert!(format(&out).is_ok(), "{text:?} -> {out:?}");
    }
});
