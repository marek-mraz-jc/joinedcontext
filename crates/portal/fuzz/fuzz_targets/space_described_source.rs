//! The source an uploaded README or `.env` describes (`jc_space_complete`): any text yields a
//! source or none, never a panic, and a secret the `.env` holds becomes a `secretRef`, never a
//! value in the spec.
#![no_main]

use joinedcontext_portal::ops::space_complete::{source_in_env, source_in_readme};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = source_in_readme(&text);
    if let Some((kind, spec, _)) = source_in_env(&text, "space") {
        assert_eq!(spec["type"], kind.as_str(), "the spec is of the kind named");
        if let Some(password) = spec.pointer("/mqtt/password") {
            assert!(password.get("secretRef").is_some(), "{password}");
        }
    }
});
