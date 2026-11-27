//! The workbench's sample and try inputs (`jc_pipeline_sample_source`, `jc_pipeline_try_mapping`):
//! no input makes their checks panic, and an accepted try names a candidate.
#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    if let Some(op) = joinedcontext_portal::ops::find("jc_pipeline_sample_source") {
        let _ = (op.validate)(&input);
    }
    if let Some(op) = joinedcontext_portal::ops::find("jc_pipeline_try_mapping") {
        if (op.validate)(&input).is_ok() {
            assert!(
                input.get("pipeline").is_some() || input.get("draft").is_some(),
                "{input}"
            );
        }
    }
});
