//! Any YAML text as a manifest of any kind: `validate_yaml` (what `jcctl validate` and the
//! Portal import wizard call) answers a result and never panics (T-7766).
#![no_main]

use jc_core::registry::{validate_yaml, KINDS};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&pick, rest)) = data.split_first() else {
        return;
    };
    let Ok(yaml) = std::str::from_utf8(rest) else {
        return;
    };
    let kind = KINDS[usize::from(pick) % KINDS.len()].kind;
    assert!(
        validate_yaml(kind, yaml).is_some(),
        "{kind} is in the catalogue"
    );
});
