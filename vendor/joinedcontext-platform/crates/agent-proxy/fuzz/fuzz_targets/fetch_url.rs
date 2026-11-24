//! The fetch route's URL and a redirect's `Location`, any text: what the rules let through is
//! https, on the profile's list and carries no credential, every hop included (T-7795).
#![no_main]

use agent_proxy::routes::fetch::fuzzing;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut parts = text.splitn(3, '\n');
    let raw = parts.next().unwrap_or_default();
    let location = parts.next().unwrap_or_default();
    let mut allowed: Vec<String> = parts
        .next()
        .unwrap_or_default()
        .split(',')
        .map(str::to_owned)
        .collect();
    allowed.push("docs.rs".to_owned());
    fuzzing::url(raw, location, &allowed);
});
