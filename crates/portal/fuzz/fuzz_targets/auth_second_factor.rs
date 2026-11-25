//! Whether a token's `acr` and `amr` prove a second factor (`auth::session::proves_second_factor`)
//! over an arbitrary `acr` and any JSON `amr`: never panics, and an `amr` that names no `otp`
//! anywhere proves one only through the step-up `acr`.
#![no_main]
use joinedcontext_portal::auth::session::{proves_second_factor, STEP_UP_ACR};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 8 * 1024 {
        return;
    }
    // The first 0xff byte (never in UTF-8) splits `acr` from the `amr` JSON.
    let (acr, amr) = match data.iter().position(|b| *b == 0xff) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &data[..0]),
    };
    let acr = std::str::from_utf8(acr).ok();
    let amr: serde_json::Value = serde_json::from_slice(amr).unwrap_or_default();
    let proved = proves_second_factor(acr, &amr);
    if acr == Some(STEP_UP_ACR) {
        assert!(proved);
    } else if !amr.to_string().contains("\"otp\"") {
        assert!(!proved, "{acr:?} {amr}");
    }
});
