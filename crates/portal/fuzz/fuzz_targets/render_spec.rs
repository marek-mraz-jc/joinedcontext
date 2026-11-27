//! `jc_render`'s input: a spec it accepts is an object naming one read, a saved widget or a
//! whole Dashboard, and no input makes the check panic.
#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let Some(op) = joinedcontext_portal::ops::find("jc_render") else {
        return;
    };
    if (op.validate)(&input).is_ok() {
        let data = &input["data"];
        assert!(input.is_object(), "{input}");
        assert!(
            ["query", "widget", "dashboard"]
                .iter()
                .filter(|key| data.get(**key).is_some())
                .count()
                == 1,
            "{input}"
        );
    }
});
