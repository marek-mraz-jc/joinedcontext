//! Input: one option byte, then the log line. Bit 0 picks RFC 3164 over RFC 5424, bit 1 best_effort,
//! bit 2 allow_rfc3339, bit 3 an empty default_year (else 2024), bit 4 no default_timezone (else
//! Europe/Bratislava). Every line ends in a document or an error, and go-syslog's RFC 5424 parser
//! answers each line with a message, an error or both (parse_log's "no message" arm for RFC 5424
//! is defensive: this target holds that it is never taken).
#![no_main]

use ekiben_text::go_syslog::rfc5424;
use ekiben_text::processor_parse_log::{ParseLogConfig, ParseLogProcessor};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&opts, line)) = data.split_first() else {
        return;
    };
    let bit = |n: u8| opts & (1 << n) != 0;
    let c = ParseLogConfig {
        format: if bit(0) {
            "syslog_rfc3164"
        } else {
            "syslog_rfc5424"
        }
        .to_string(),
        best_effort: bit(1),
        with_rfc3339: bit(2),
        with_year: if bit(3) { "" } else { "2024" }.to_string(),
        with_timezone: if bit(4) { "" } else { "Europe/Bratislava" }.to_string(),
    };
    if !bit(0) {
        let (m, e) = rfc5424::parse(line, bit(1));
        assert!(m.is_some() || e.is_some(), "neither message nor error");
    }
    let p = ParseLogProcessor::new(&c).expect("a fixed config builds");
    if let Ok(doc) = p.parse(line) {
        assert_eq!(doc.type_name(), "object", "parse_log writes an object");
    }
});
