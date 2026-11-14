//! `skip_bom` scanner (`internal/impl/pure/scanner_skip_bom.go`): drops one byte order mark from
//! the start of the stream, the longest that matches, before the child scanner (`into`) reads it.

/// Bento's marks: UTF-32 BE and LE, UTF-8, UTF-16 BE and LE.
const BOMS: [&[u8]; 5] = [
    &[0x00, 0x00, 0xFE, 0xFF],
    &[0xFF, 0xFE, 0x00, 0x00],
    &[0xEF, 0xBB, 0xBF],
    &[0xFE, 0xFF],
    &[0xFF, 0xFE],
];

/// The stream after its byte order mark.
pub fn skip_bom(raw: &[u8]) -> &[u8] {
    skip_group(raw, &BOMS)
}

/// Bento's `skipGroup`: the longest group the stream starts with is cut off; none, the stream stays.
pub fn skip_group<'a>(raw: &'a [u8], groups: &[&[u8]]) -> &'a [u8] {
    let mut sorted = groups.to_vec();
    sorted.sort_by_key(|g| std::cmp::Reverse(g.len()));
    sorted
        .into_iter()
        .find(|g| raw.starts_with(g))
        .map_or(raw, |g| &raw[g.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bento `TestReadUpTo`; the Go test's one-byte reader has no twin here, the stream is whole.
    #[test]
    fn test_read_up_to() {
        let cases: [(&str, &str, &[&[u8]], &str); 7] = [
            ("no groups", "foo", &[], "foo"),
            ("no input", "", &[b"hello "], ""),
            (
                "an empty group",
                "hello world",
                &[b"not this", b""],
                "hello world",
            ),
            ("easy match", "hello world", &[b"hello "], "world"),
            ("exact skip match", "foo", &[b"foo"], ""),
            ("max is bigger", "foa", &[b"fo", b"what this is huge"], "a"),
            (
                "order doesnt matter",
                "helloworld",
                &[b"hellowoa", b"hella", b"hello", b"hellowor", b"hea"],
                "ld",
            ),
        ];
        for (name, input, skips, expected) in cases {
            assert_eq!(
                skip_group(input.as_bytes(), skips),
                expected.as_bytes(),
                "{name}"
            );
        }
    }

    #[test]
    fn skips_each_bento_mark() {
        for bom in BOMS {
            let mut raw = bom.to_vec();
            raw.extend_from_slice(b"hello");
            assert_eq!(skip_bom(&raw), b"hello");
        }
        assert_eq!(skip_bom(b"\xEF\xBBhello"), b"\xEF\xBBhello");
    }
}
