//! Input: the time text, a NUL byte, then the layout. Without a NUL the whole input is both.
//! Parses with the Go layout and the strptime format, formats each result back with the same
//! layout, and reads the text as a Go duration and an ISO 8601 period.
#![no_main]

use ekiben_bloblang::time_format::{
    format_go_layout, format_strftime, parse_go_duration_nanos, parse_go_layout, parse_strptime,
    Iso8601Period,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (text, layout) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, data),
    };
    let text = String::from_utf8_lossy(text);
    let layout = String::from_utf8_lossy(layout);
    if let Ok(dt) = parse_go_layout(&text, &layout) {
        let _ = format_go_layout(&dt, &layout);
    }
    if let Ok(dt) = parse_strptime(&text, &layout) {
        let _ = format_strftime(&dt, &layout);
        if let Ok(period) = Iso8601Period::parse(&text) {
            let _ = period.approx_nanos();
            let _ = period.negate().add_to(&dt);
        }
    }
    let _ = parse_go_duration_nanos(&text);
});
