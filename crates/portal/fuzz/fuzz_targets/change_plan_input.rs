//! The inputs of `jc_change_plan`, `jc_change_revert_plan` and `jc_change_apply`: one each accepts
//! is an object naming its changes, the change to undo or the plan to apply, and no input makes
//! a check panic.
#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    for (name, member) in [
        ("jc_change_plan", "changes"),
        ("jc_change_revert_plan", "id"),
        ("jc_change_apply", "planId"),
    ] {
        let Some(op) = joinedcontext_portal::ops::find(name) else {
            continue;
        };
        if (op.validate)(&input).is_ok() {
            assert!(input.get(member).is_some(), "{name}: {input}");
        }
    }
});
