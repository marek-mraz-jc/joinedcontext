//! Input: a source URL or a redirect Location as an editor or a remote writes it (first line),
//! and a source's cron schedule (second line). An accepted URL is always https, without
//! credentials, on a listed host; the schedule reads as fires / does not fire / refused, never
//! a panic.
#![no_main]

use dataroom_harvest::{cron_matches, validate_outbound_url};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let (url, schedule) = text.split_once('\n').unwrap_or((text, ""));
    let allowed = ["data.example.org".to_owned(), "*.city.example".to_owned()];
    let catalogue = ["catalogue.example".to_owned()];
    if let Ok(parsed) = validate_outbound_url(url, &allowed, &catalogue, false) {
        assert_eq!(parsed.scheme(), "https", "{url}");
        assert!(parsed.username().is_empty() && parsed.password().is_none(), "{url}");
        assert!(parsed.host_str().is_some(), "{url}");
    }
    let at = serde_json::from_str("\"2026-11-08T03:23:00Z\"").expect("timestamp");
    let _ = cron_matches(schedule, at);
});
