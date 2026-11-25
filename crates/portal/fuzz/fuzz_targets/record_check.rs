//! Input: `{"pipeline": <a Pipeline manifest>, "records": [<records>]}`, as the pipeline test
//! door takes them from a person (T-7729): the check answers findings or a 400 naming the
//! spec, never a panic, and judges at most its cap of records.
#![no_main]

use std::sync::OnceLock;

use joinedcontext_portal::config::Config;
use joinedcontext_portal::record_check::check_records;
use joinedcontext_portal::state::AppState;
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

static STATE: OnceLock<AppState> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let Ok(input) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let records = input["records"].as_array().cloned().unwrap_or_default();
    let state = STATE.get_or_init(|| AppState::new(Config::for_tests(), None));
    let now = jc_time::TimestampTz::now();
    match check_records(state, "helsinki", &input["pipeline"], &records, now) {
        Ok(checked) => assert!(checked.records.len() <= records.len()),
        Err(refused) => assert!(!refused.to_string().trim().is_empty()),
    }
});
