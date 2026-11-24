//! Input: a config text with `${VAR}`, `${VAR:default}`, `${{escaped}}` and `${! bloblang }`
//! forms. Interpolation answers the text or the missing names; a text without `${` comes back
//! unchanged, and every missing name is a valid variable name listed once.
#![no_main]

use ekiben_core::{interpolate_env_bytes, interpolate_env_str};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let lookup = |name: &str| (name.len() % 2 == 0).then(|| format!("<{name}>"));
    let _ = interpolate_env_bytes(data, lookup);
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    match interpolate_env_str(text, lookup) {
        Ok(out) if !text.contains("${") => assert_eq!(out, text),
        Ok(_) => {}
        Err(e) => {
            for (i, name) in e.missing.iter().enumerate() {
                assert!(!name.is_empty() && name.len() % 2 == 1, "{name}");
                assert!(!e.missing[..i].contains(name), "{name} listed twice");
            }
        }
    }
});
