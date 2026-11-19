// SPDX-License-Identifier: EUPL-1.2
//! iCalendar TEXT values, shared by the event mapping and the `VTIMEZONE` writer.

/// RFC 5545 3.3.11 TEXT: backslash, semicolon, comma and newline escaped; the other control
/// characters, which TEXT excludes, dropped.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}
