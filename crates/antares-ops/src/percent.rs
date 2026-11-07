// SPDX-License-Identifier: EUPL-1.2
//! Percent-coding (RFC 3986 2.1) as every surface reads and writes it: one
//! copy, so a surface cannot decode a path the others refuse (T-4020).

use std::fmt::Write as _;

/// `raw` with everything but the unreserved characters (2.3) as `%XX`.
#[must_use]
pub fn encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// `text` with each `%XX` decoded once; `None` for a broken escape or
/// bytes that are no UTF-8.
#[must_use]
pub fn decoded(text: &str) -> Option<String> {
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(b) = bytes.next() {
        out.push(if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?
        } else {
            b
        });
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_all_but_unreserved_and_decodes_back() {
        assert_eq!(encode("a b/ü~"), "a%20b%2F%C3%BC~");
        assert_eq!(decoded(&encode("a b/ü~")).as_deref(), Some("a b/ü~"));
        assert_eq!(decoded("%2f%2F").as_deref(), Some("//"));
        assert_eq!(decoded("").as_deref(), Some(""));
    }

    #[test]
    fn refuses_a_broken_escape_or_no_utf8() {
        for broken in ["%", "%2", "%zz", "%+1", "a%2", "%FF"] {
            assert_eq!(decoded(broken), None, "{broken}");
        }
        // decoded once: an escaped percent stays an escape
        assert_eq!(decoded("%2541").as_deref(), Some("%41"));
    }
}
