// SPDX-License-Identifier: EUPL-1.2
//! The connection id and resume token (ADR-0026 rules 4 and 7): random
//! bits from the operating system, never a counter or a clock.

use data_encoding::BASE32_NOPAD;

fn bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // An operating system without randomness cannot mint an id nobody can
    // guess; the all-zero fallback would make every id equal, so the
    // process stops instead (the same rule uuid v4 follows).
    if getrandom::fill(&mut b).is_err() {
        std::process::abort();
    }
    b
}

/// 128 random bits, base32 (rule 4), lowercase so it reads as one word.
pub(crate) fn id() -> String {
    BASE32_NOPAD.encode(&bytes::<16>()).to_ascii_lowercase()
}

/// 256 random bits, base32 (rule 7).
pub(crate) fn token() -> String {
    BASE32_NOPAD.encode(&bytes::<32>()).to_ascii_lowercase()
}

/// Equal bytes, in time that depends on the length alone.
pub(crate) fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_tokens_are_fresh_and_compare_whole() {
        assert_eq!(id().len(), 26);
        assert_eq!(token().len(), 52);
        assert_ne!(id(), id());
        assert!(equal(b"abc", b"abc"));
        assert!(!equal(b"abc", b"abd"));
        assert!(!equal(b"abc", b"ab"));
    }
}
