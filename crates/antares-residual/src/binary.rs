// SPDX-License-Identifier: EUPL-1.2
//! Base64 text of a binary value (RFC 4648): NGSI-LD stores one as a string, in the standard or
//! the URL alphabet, padded or not; `OData` writes `Edm.Binary` as unpadded base64url (JSON
//! Format 7.1).

/// The bytes `text` encodes in either alphabet (RFC 4648 4 and 5), padded with `=` to a
/// multiple of four or unpadded; `None` for any other character, misplaced padding, a length
/// of one past a multiple of four or nonzero bits past the last byte.
///
/// ```
/// use ngsild_residual::decode_base64;
///
/// assert_eq!(decode_base64("dGVzdA=="), Some(b"test".to_vec()));
/// assert_eq!(decode_base64("dGVzdA"), Some(b"test".to_vec()));
/// assert_eq!(decode_base64("-_8"), decode_base64("+/8="));
/// assert_eq!(decode_base64("!!"), None);
/// ```
#[must_use]
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let data = text.trim_end_matches('=');
    let padding = text.len().saturating_sub(data.len());
    if padding > 2 || (padding > 0 && !text.len().is_multiple_of(4)) || data.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(data.len().saturating_mul(3) / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for b in data.bytes() {
        acc = (acc << 6) | u32::from(sextet(b)?);
        bits = bits.saturating_add(6);
        if bits >= 8 {
            bits = bits.saturating_sub(8);
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
            acc &= 1u32.checked_shl(bits)?.wrapping_sub(1);
        }
    }
    (acc == 0).then_some(out)
}

/// `bytes` as unpadded base64url (RFC 4648 5).
///
/// ```
/// use ngsild_residual::base64url;
///
/// assert_eq!(base64url(b"test"), "dGVzdA");
/// assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
/// ```
#[must_use]
pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    let mut push = |sextet: u8| {
        out.extend(
            ALPHABET
                .get(usize::from(sextet & 0x3f))
                .map(|b| char::from(*b)),
        );
    };
    for chunk in bytes.chunks(3) {
        let [b0, b1, b2] = [0, 1, 2].map(|i| chunk.get(i).copied());
        let (b0, low) = (b0.unwrap_or_default(), b1.unwrap_or_default());
        push(b0 >> 2);
        push((b0 << 4) | (low >> 4));
        if b1.is_some() {
            push((low << 2) | (b2.unwrap_or_default() >> 6));
        }
        if let Some(b2) = b2 {
            push(b2);
        }
    }
    out
}

fn sextet(b: u8) -> Option<u8> {
    Some(match b {
        b'A'..=b'Z' => b.wrapping_sub(b'A'),
        b'a'..=b'z' => b.wrapping_sub(b'a' - 26),
        b'0'..=b'9' => b.wrapping_add(52 - b'0'),
        b'+' | b'-' => 62,
        b'/' | b'_' => 63,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{base64url, decode_base64};

    #[test]
    fn every_length_round_trips_and_bad_text_is_none() {
        let bytes: Vec<u8> = (0..=255).collect();
        for len in 0..bytes.len() {
            let slice = bytes.get(..len).unwrap_or_default();
            assert_eq!(decode_base64(&base64url(slice)).as_deref(), Some(slice));
        }
        assert_eq!(decode_base64(""), Some(Vec::new()));
        assert_eq!(decode_base64("AQID"), Some(vec![1, 2, 3]));
        for bad in [
            "A",
            "AB=",
            "A===",
            "AB=C",
            "dGVzdB==",
            "!!!invalid!!!",
            "AQ ID",
        ] {
            assert_eq!(decode_base64(bad), None, "{bad}");
        }
    }
}
