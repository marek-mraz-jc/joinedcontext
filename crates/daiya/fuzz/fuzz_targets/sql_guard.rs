//! Any text as a dashboard's SQL query: the read-only guard answers the SQL it would run or a
//! refusal, never a panic or a hang, and what it lets through checks again unchanged.
#![no_main]

use daiya_query::postgres::validate_readonly_select;
use libfuzzer_sys::fuzz_target;
use std::collections::BTreeSet;

fuzz_target!(|data: &[u8]| {
    let Ok(sql) = std::str::from_utf8(data) else {
        return;
    };
    let views = BTreeSet::from(["org_metrics_view".to_owned(), "reports.daily".to_owned()]);
    if let Ok(checked) = validate_readonly_select("A", sql, &views) {
        assert!(
            validate_readonly_select("A", &checked, &views).is_ok(),
            "the checked SQL is refused on a second check: {checked:?}"
        );
    }
});
