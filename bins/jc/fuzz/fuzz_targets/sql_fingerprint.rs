//! Any SQL text: the slow-statement fingerprint never panics, keeps at most 120 characters and
//! carries no quoted literal (T-7634, T-7479).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(sql) = std::str::from_utf8(data) else {
        return;
    };
    let print = jc::sql_fingerprint(sql);
    assert!(print.chars().count() <= 120, "{print}");
    assert!(!print.contains('\''), "{print}");
    assert_eq!(print, print.trim_end());
});
