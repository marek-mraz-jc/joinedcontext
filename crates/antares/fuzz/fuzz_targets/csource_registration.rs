//! 5.2.9 Context Source Registration bodies through the create and patch
//! validators: never panic, and what passes keeps the invariants
//! `antares_api::fuzzing::csource_registration` asserts.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // the information caps bound the work per body; a longer input finds
    // nothing a 64 KiB one cannot
    if data.len() > 65_536 {
        return;
    }
    let _ = antares_api::fuzzing::csource_registration(data);
});
