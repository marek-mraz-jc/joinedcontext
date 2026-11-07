// SPDX-License-Identifier: EUPL-1.2
//! Patterns to linear-time [`Regex`] programs (ADR-0028 rule 7): `OData` `matchesPattern`
//! (an ECMAScript pattern, URL Conventions 5.1.1.7.1) and CQL2 `LIKE` (`%`, `_`, `\`).
//!
//! An ECMAScript pattern is translated construct by construct into the `regex` syntax with the
//! same meaning. Whatever has no such twin is [`Error::Unsupported`], never a different match:
//! look-around and back-references (no linear-time engine has them), `\b`/`\B` (the two disagree
//! on what a word character is), and every escape, group form or class syntax this file does not
//! name. `regex` refusing the translation is [`Error::Unsupported`] too, because a pattern
//! ECMAScript reads and `regex` cannot is not the caller's mistake to fix.

use regex::{Regex, RegexBuilder};

use crate::Error;

/// The longest pattern text, in bytes.
pub const MAX_PATTERN_BYTES: usize = 4096;
/// The ceiling on one compiled program (the bound `antares-ql` puts on a `q` pattern): program
/// size follows counted repetition, not length, so `(?:a{100}){100}` is refused here.
pub const MAX_PROGRAM_BYTES: usize = 256 * 1024;

/// What ECMAScript's `\d`, `\w` and `\s` and their negations match, inside a class.
const DIGIT: &str = "0-9";
const WORD: &str = "0-9A-Za-z_";
const SPACE: &str = r"\s\x{FEFF}";

/// An ECMAScript pattern, unanchored as `matchesPattern` reads it.
pub fn ecmascript(pattern: &str) -> Result<Regex, Error> {
    if pattern.len() > MAX_PATTERN_BYTES {
        return Err(Error::TooComplex);
    }
    build(&translate(pattern)?)
}

/// A CQL2 `LIKE` pattern, matched against the whole value: `%` any run of characters, `_` one
/// character, `\` takes the next character literally.
pub fn like(pattern: &str) -> Result<Regex, Error> {
    if pattern.len() > MAX_PATTERN_BYTES {
        return Err(Error::TooComplex);
    }
    let mut out = String::from("(?s)^");
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '%' => out.push_str(".*"),
            '_' => out.push('.'),
            '\\' => {
                let next = chars
                    .next()
                    .ok_or_else(|| Error::Invalid("a LIKE pattern ends in an escape".into()))?;
                out.push_str(&regex::escape(next.encode_utf8(&mut [0; 4])));
            }
            other => out.push_str(&regex::escape(other.encode_utf8(&mut [0; 4]))),
        }
    }
    out.push('$');
    build(&out)
}

fn build(source: &str) -> Result<Regex, Error> {
    RegexBuilder::new(source)
        .size_limit(MAX_PROGRAM_BYTES)
        .dfa_size_limit(MAX_PROGRAM_BYTES)
        .build()
        .map_err(|e| match e {
            regex::Error::CompiledTooBig(_) => Error::TooComplex,
            _ => Error::Unsupported(
                "a pattern construct the linear-time engine reads differently".into(),
            ),
        })
}

fn unsupported(what: &str) -> Error {
    Error::Unsupported(format!("{what} in a pattern"))
}

/// The `regex` source with the meaning `pattern` has in ECMAScript.
fn translate(pattern: &str) -> Result<String, Error> {
    let mut out = String::with_capacity(pattern.len().saturating_add(16));
    let mut chars = pattern.chars().peekable();
    let mut class = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let e = chars
                    .next()
                    .ok_or_else(|| unsupported("a trailing escape"))?;
                if matches!(e, 'x' | 'u') && chars.peek() == Some(&'{') {
                    return Err(unsupported("a braced code point"));
                }
                escape(e, class, &mut out)?;
            }
            '[' if class => return Err(unsupported("a nested class")),
            '[' => {
                class = true;
                out.push('[');
                if chars.peek() == Some(&'^') {
                    chars.next();
                    out.push('^');
                }
                if chars.peek() == Some(&']') {
                    return Err(unsupported("an empty class"));
                }
            }
            ']' if class => {
                class = false;
                out.push(']');
            }
            '&' | '-' | '~' if class && chars.peek() == Some(&c) => {
                return Err(unsupported("a class set operation"));
            }
            '.' if !class => out.push_str(r"[^\n\r\x{2028}\x{2029}]"),
            '(' if !class && chars.peek() == Some(&'?') => {
                chars.next();
                match chars.next() {
                    Some(':') => out.push_str("(?:"),
                    Some('<') if chars.peek().is_some_and(char::is_ascii_alphabetic) => {
                        out.push_str("(?<");
                    }
                    Some('=' | '!' | '<') => return Err(unsupported("look-around")),
                    _ => return Err(unsupported("a group flag")),
                }
            }
            '{' if !class && chars.peek() == Some(&',') => {
                return Err(unsupported("an open lower bound"))
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// One escape `\e`, inside or outside a class.
fn escape(e: char, class: bool, out: &mut String) -> Result<(), Error> {
    match e {
        'd' | 'w' | 's' | 'D' | 'W' | 'S' => {
            let inner = match e.to_ascii_lowercase() {
                'd' => DIGIT,
                'w' => WORD,
                _ => SPACE,
            };
            match (class, e.is_ascii_uppercase()) {
                (true, true) => return Err(unsupported("a negated shorthand inside a class")),
                (true, false) => out.push_str(inner),
                (false, negated) => {
                    out.push_str(if negated { "[^" } else { "[" });
                    out.push_str(inner);
                    out.push(']');
                }
            }
        }
        'b' | 'B' => return Err(unsupported("a word boundary")),
        '1'..='9' | 'k' => return Err(unsupported("a back-reference")),
        e if matches!(e, 't' | 'n' | 'r' | 'f' | 'v' | 'x' | 'u') || e.is_ascii_punctuation() => {
            out.push('\\');
            out.push(e);
        }
        _ => return Err(unsupported("an escape")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ecmascript, like, MAX_PATTERN_BYTES};
    use crate::Error;

    fn matches(pattern: &str, value: &str) -> bool {
        ecmascript(pattern).is_ok_and(|r| r.is_match(value))
    }

    #[test]
    fn shorthands_keep_their_ecmascript_meaning() {
        assert!(matches(r"^\d+$", "0123"));
        assert!(!matches(r"^\d$", "٣"), "ECMAScript \\d is ASCII");
        assert!(!matches(r"^\w$", "é"), "ECMAScript \\w is ASCII");
        assert!(matches(r"^\W$", "é"));
        assert!(matches(r"^\D$", "x"));
        assert!(matches(r"^\s$", "\u{FEFF}"));
        assert!(matches(r"^\S$", "a"));
        assert!(matches(r"^[\d_]+$", "1_2"));
        assert!(matches(r"^[^\w]$", "-"));
        for line_end in ["\n", "\r", "\u{2028}"] {
            assert!(!matches(r"^.$", line_end));
        }
        assert!(matches(r"^.$", "é"));
        assert!(matches(r"^[.]$", "."));
        assert!(matches(r"^a\tb\.c\x41B$", "a\tb.cAB"));
        assert!(matches(r"^(?:ab)+(?<n>c)$", "ababc"));
        assert!(matches(r"^a{2,3}$", "aaa"));
        assert!(matches(r"^[a-c]\/\-$", "b/-"));
        assert!(
            matches(r"x", "axb"),
            "unanchored, as matchesPattern reads it"
        );
        assert!(
            matches(r"^a]-b~&$", "a]-b~&"),
            "outside a class these are literals"
        );
        assert!(matches(r"^[(][{][&][~]$", "({&~"), "inside a class too");
    }

    #[test]
    fn constructs_without_a_twin_are_refused_not_reread() {
        for pattern in [
            r"a(?=b)",
            r"a(?!b)",
            r"(?<=a)b",
            r"(?<!a)b",
            r"(a)\1",
            r"(?<n>a)\k<n>",
            r"\bword\b",
            r"\B",
            r"(?i)a",
            r"\p{L}",
            r"\A",
            r"\cJ",
            r"[[:alpha:]]",
            r"[a&&b]",
            r"[a--b]",
            r"[a~~b]",
            r"[]",
            r"[^]",
            r"[\D]",
            r"a{,3}",
            r"\u{41}",
            r"\x{41}",
            "a\\",
            r"(",
            r"a{2,1}",
        ] {
            assert!(
                matches!(ecmascript(pattern), Err(Error::Unsupported(_))),
                "{pattern}"
            );
        }
    }

    #[test]
    fn size_is_bounded_before_and_after_compiling() {
        assert_eq!(
            ecmascript(&"a".repeat(MAX_PATTERN_BYTES + 1)).err(),
            Some(Error::TooComplex)
        );
        assert_eq!(
            like(&"a".repeat(MAX_PATTERN_BYTES + 1)).err(),
            Some(Error::TooComplex)
        );
        assert_eq!(
            ecmascript(r"(?:[a-z]{100}){100}").err(),
            Some(Error::TooComplex)
        );
    }

    #[test]
    fn like_wildcards_escapes_and_whole_value() {
        let is = |p: &str, v: &str| like(p).is_ok_and(|r| r.is_match(v));
        assert!(is("Sta%", "Station 1"));
        assert!(!is("Sta%", "A Station"));
        assert!(is("a_c", "abc"));
        assert!(!is("a_c", "abbc"));
        assert!(is("%", "line\nbreak"));
        assert!(is(r"100\%", "100%"));
        assert!(!is(r"100\%", "1000"));
        assert!(is(r"a\_b", "a_b"));
        assert!(!is(r"a\_b", "axb"));
        assert!(is("(.)*", "(.)*"), "regex syntax is literal");
        assert!(!is("(.)*", "(x)*"));
        assert!(matches!(like("abc\\"), Err(Error::Invalid(_))));
    }
}
