//! `re_match` scanner (`internal/impl/pure/scanner_re_match.go`): splits the stream at the
//! matches of `pattern`, each segment starting with its match, as Bento's `bufio` split function.

use bytes::Bytes;
use ekiben_core::CoreError;
use serde_json::Value as JsonValue;

/// `bufio.MaxScanTokenSize`, Bento's default `max_buffer_size`.
const MAX_SCAN_TOKEN_SIZE: u64 = 64 * 1024;
/// Compiled program cap: a pattern that would compile past it is refused, not run.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

#[derive(Clone, Debug)]
pub struct ReMatchScanner {
    regex: regex::bytes::Regex,
    max_buffer_size: usize,
}

impl ReMatchScanner {
    pub fn from_config(obj: &serde_json::Map<String, JsonValue>) -> Result<Self, CoreError> {
        let pattern = obj
            .get("pattern")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("re_match: field 'pattern' is required"))?;
        let max_buffer_size = match obj.get("max_buffer_size") {
            None => MAX_SCAN_TOKEN_SIZE,
            Some(v) => v.as_u64().ok_or_else(|| {
                CoreError::config("re_match: field 'max_buffer_size' must be a positive integer")
            })?,
        };
        let regex = regex::bytes::RegexBuilder::new(pattern)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_SIZE_LIMIT)
            .build()
            .map_err(|e| CoreError::config(format!("re_match: invalid pattern: {e}")))?;
        Ok(Self {
            regex,
            max_buffer_size: usize::try_from(max_buffer_size).unwrap_or(usize::MAX),
        })
    }

    /// The segments of `data`: up to the first match, then from each match to the next one.
    pub fn split(&self, data: &[u8]) -> Result<Vec<Bytes>, CoreError> {
        let mut out = Vec::new();
        let mut rest = data;
        while !rest.is_empty() {
            let mut found = self.regex.find_iter(rest);
            let advance = match (found.next(), found.next()) {
                (None, _) => rest.len(),
                (Some(m), None) if m.start() == 0 => rest.len(),
                (Some(m), None) => m.start(),
                (Some(m0), Some(m1)) if m0.start() == 0 => m1.start(),
                (Some(m0), Some(_)) => m0.start(),
            };
            if advance > self.max_buffer_size {
                return Err(CoreError::other("bufio.Scanner: token too long"));
            }
            out.push(Bytes::copy_from_slice(&rest[..advance]));
            rest = &rest[advance..];
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanner(pattern: &str) -> ReMatchScanner {
        let conf = serde_json::json!({"pattern": pattern, "max_buffer_size": 200});
        ReMatchScanner::from_config(conf.as_object().expect("an object")).expect("a scanner")
    }

    fn check(pattern: &str, input: &str, expected: &[&str]) {
        let got: Vec<String> = scanner(pattern)
            .split(input.as_bytes())
            .expect("scanned")
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect();
        assert_eq!(got, expected, "pattern {pattern:?} on {input:?}");
    }

    /// Bento `TestReMatchScannerSuite`, every case.
    #[test]
    fn test_re_match_scanner_suite() {
        check("(?m)^", "foo\nbar\nbaz", &["foo\n", "bar\n", "baz"]);
        check(
            "split",
            "foo\nbar\nsplit\nbaz\nsplitsplit",
            &["foo\nbar\n", "split\nbaz\n", "split", "split"],
        );
        check("\\n", "split", &["split"]);
        check("split", "split", &["split"]);
        check(
            "\\n",
            "foo\nbar\nsplit\nbaz\nsplitsplit",
            &["foo", "\nbar", "\nsplit", "\nbaz", "\nsplitsplit"],
        );
        check("\\n", "foo\nbar\nsplit\nbaz", &["foo", "\nbar", "\nsplit", "\nbaz"]);
        check(
            "\\n\\d",
            "20:20:22 ERROR\nCode\n20:20:21 INFO\n20:20:21 INFO\n20:20:22 ERROR\nCode\n",
            &[
                "20:20:22 ERROR\nCode",
                "\n20:20:21 INFO",
                "\n20:20:21 INFO",
                "\n20:20:22 ERROR\nCode\n",
            ],
        );
        check(
            "(?m)^\\d\\d:\\d\\d:\\d\\d",
            "20:20:22 ERROR\nCode\n20:20:21 INFO\n20:20:21 INFO\n20:20\n20:20:22 ERROR\nCode\n2022",
            &[
                "20:20:22 ERROR\nCode\n",
                "20:20:21 INFO\n",
                "20:20:21 INFO\n20:20\n",
                "20:20:22 ERROR\nCode\n2022",
            ],
        );
        check("split", "", &[]);
    }

    #[test]
    fn a_segment_over_max_buffer_size_fails() {
        let long = "x".repeat(201);
        let err = scanner("split").split(long.as_bytes()).expect_err("too long");
        assert!(err.to_string().contains("token too long"), "{err}");
    }

    #[test]
    fn a_pattern_past_the_compile_limit_is_refused() {
        let conf = serde_json::json!({"pattern": "(\\w{100}){100}"});
        let err = ReMatchScanner::from_config(conf.as_object().expect("an object"))
            .expect_err("refused");
        assert!(err.to_string().contains("invalid pattern"), "{err}");
    }
}
