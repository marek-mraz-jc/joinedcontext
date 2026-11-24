//! Any text as a cron expression: the checks answer and never panic, and what `is_valid` accepts
//! `matches` reads at every minute it is asked (T-7766).
#![no_main]

use jc_core::cron;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&at, rest)) = data.split_first() else {
        return;
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let at = u32::from(at);
    let read = cron::matches(text, at % 60, at % 24, at % 31 + 1, at % 12 + 1, at % 8);
    assert_eq!(read.is_some(), cron::is_valid(text), "{text:?}");
    if let Some(gap) = cron::shortest_gap_minutes(text) {
        assert!(gap <= 24 * 60, "{text:?} {gap}");
    }
});
