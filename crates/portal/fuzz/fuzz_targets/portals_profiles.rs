//! `JC_PORTAL_PROFILES`: any text is a list of profiles or a sentence, and never a panic.
#![no_main]

use joinedcontext_portal::portals::parse_profiles;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    match parse_profiles(text) {
        Ok(profiles) => {
            if text.trim().is_empty() {
                assert!(profiles.is_empty());
            }
        }
        Err(reason) => assert!(reason.starts_with("not a list of profiles"), "{reason}"),
    }
});
