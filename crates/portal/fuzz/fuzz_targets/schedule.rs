//! A pipeline's cron `schedule` from its manifest: a period in seconds or none, never zero.
#![no_main]

use joinedcontext_portal::quality::{schedule_seconds, target_seconds};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Some(seconds) = schedule_seconds(text) {
            assert!(seconds > 0, "{text:?} read as a zero period");
        }
        let _ = target_seconds(None, Some(text));
    }
});
