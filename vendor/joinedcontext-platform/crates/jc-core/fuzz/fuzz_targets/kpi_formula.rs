//! Any text as a KPI formula: the parser answers and never panics, and the earlier reads of what
//! it parses are listed without a panic either (T-7766).
#![no_main]

use jc_core::kpi_join::{earlier_reads, parse, period_seconds, period_words};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(expr) = parse(text) {
        let _ = earlier_reads(&expr);
    }
    let _ = period_seconds(text);
    let _ = period_words(text);
});
