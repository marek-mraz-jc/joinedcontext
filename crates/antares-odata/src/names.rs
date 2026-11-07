// SPDX-License-Identifier: EUPL-1.2
//! NGSI-LD and `LinkML` names as CSDL simple identifiers
//! (odata-linkml-mapping.md 3.3).
//!
//! A name that is a simple identifier stays as it is. Every code point CSDL
//! XML 15.2 does not allow where it stands is written `_xHHHH_` (uppercase
//! hex, at least four digits), and so is the `_` that would start a literal
//! `_x<hex>_` in the source, so two names never map to one. A result over
//! 128 characters has no CSDL name.

/// The CSDL simple identifier of `name`, `None` when it would be longer than
/// 128 characters.
pub fn mangle(name: &str) -> Option<String> {
    let mut out = String::with_capacity(name.len());
    let mut chars = 0usize;
    for (i, c) in name.char_indices() {
        let first = i == 0;
        let keep =
            allowed(c, first) && !(c == '_' && starts_escape(name.get(i..).unwrap_or_default()));
        if keep {
            out.push(c);
            chars = chars.saturating_add(1);
        } else {
            let escaped = format!("_x{:04X}_", u32::from(c));
            chars = chars.saturating_add(escaped.len());
            out.push_str(&escaped);
        }
    }
    (chars > 0 && chars <= 128).then_some(out)
}

/// Whether `c` may stand where it does in a simple identifier.
fn allowed(c: char, first: bool) -> bool {
    let mut probe = String::from(if first { "" } else { "a" });
    probe.push(c);
    odata_csdl::is_simple_identifier(&probe)
}

/// Whether `rest` begins `_x`, one or more hex digits, then `_`.
fn starts_escape(rest: &str) -> bool {
    let Some(tail) = rest.strip_prefix("_x") else {
        return false;
    };
    let digits = tail.bytes().take_while(u8::is_ascii_hexdigit).count();
    digits > 0 && tail.as_bytes().get(digits) == Some(&b'_')
}

#[cfg(test)]
mod tests {
    use super::mangle;

    #[test]
    fn a_simple_identifier_stays() {
        for name in ["temperature", "Room", "_types", "a1", "Größe"] {
            assert_eq!(mangle(name).as_deref(), Some(name));
        }
    }

    #[test]
    fn every_other_code_point_is_escaped() {
        assert_eq!(mangle("ex:speed").as_deref(), Some("ex_x003A_speed"));
        assert_eq!(mangle("pm2.5").as_deref(), Some("pm2_x002E_5"));
        assert_eq!(mangle("1st").as_deref(), Some("_x0031_st"));
        assert_eq!(mangle("m²").as_deref(), Some("m_x00B2_"));
        assert_eq!(mangle("cars-1").as_deref(), Some("cars_x002D_1"));
    }

    #[test]
    fn a_literal_escape_in_the_source_is_escaped_so_the_mapping_is_injective() {
        let literal = mangle("ex_x003A_speed");
        assert_eq!(literal.as_deref(), Some("ex_x005F_x003A_speed"));
        assert_ne!(literal, mangle("ex:speed"));
        assert_eq!(
            mangle("a_x_b").as_deref(),
            Some("a_x_b"),
            "no hex digit: no escape"
        );
        assert_eq!(
            mangle("a_x1").as_deref(),
            Some("a_x1"),
            "no closing underscore: no escape"
        );
    }

    #[test]
    fn past_128_characters_there_is_no_name() {
        assert!(mangle(&"a".repeat(128)).is_some());
        assert!(mangle(&"a".repeat(129)).is_none());
        assert!(
            mangle(&format!("{}:", "a".repeat(122))).is_none(),
            "122 + 7"
        );
        assert!(
            mangle(&format!("{}:", "a".repeat(121))).is_some(),
            "121 + 7"
        );
        assert!(mangle("").is_none());
    }
}
