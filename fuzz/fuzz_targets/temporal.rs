//! 4.11 temporal query and representation parameters (`timerel`,
//! `timeAt`, `endTimeAt`, `timeproperty`, `lastN`, `aggrMethods`,
//! `aggrPeriodDuration`) and the ISO 8601 durations they carry: never
//! panics (T-3463).
#![no_main]
use libfuzzer_sys::fuzz_target;

/// `key=value` lines into query parameters, as the API receives them.
fn params(text: &str) -> std::collections::HashMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

fuzz_target!(|data: &[u8]| {
    // the API caps a query string and a header far below this, so no
    // allocation the target sees scales past what a request can carry
    if data.len() > 4096 {
        return;
    }
    if let Ok(text) = std::str::from_utf8(data) {
        let params = params(text);
        let _ = antares_api::fuzzing::temporal(&params);
        if let Some(duration) = params.get("aggrPeriodDuration") {
            let _ = antares_model::parse_iso_duration(duration);
        }
    }
});
