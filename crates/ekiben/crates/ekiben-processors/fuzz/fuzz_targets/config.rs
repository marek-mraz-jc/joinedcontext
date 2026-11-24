//! Input: a JSON processor config list (as a pipeline manifest carries it), a NUL byte, then the
//! message. Building may refuse; a built list must process the message without a panic.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (config, message) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &br#"{"a":1}"#[..]),
    };
    let Ok(config) = serde_json::from_slice::<serde_json::Value>(config) else {
        return;
    };
    let ctx = ekiben_processors_fuzz::ctx();
    if let Some(procs) = ekiben_processors_fuzz::build(&ctx, &config) {
        ekiben_processors_fuzz::run(&ctx, &procs, message);
    }
});
