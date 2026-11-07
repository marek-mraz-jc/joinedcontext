// SPDX-License-Identifier: EUPL-1.2
//! CSDL names (CSDL XML 15.1 to 15.3): simple identifiers, namespaces and qualified names.

use std::sync::LazyLock;

use regex::Regex;

/// `TSimpleIdentifier` of `edm.xsd` without its length bound, which [`is_simple_identifier`]
/// counts in characters (a bounded repetition of these classes would compile to a large
/// automaton).
const IDENTIFIER: &str = r"^[\p{L}\p{Nl}_][\p{L}\p{Nl}\p{Nd}\p{Mn}\p{Mc}\p{Pc}\p{Cf}]*$";

// ponytail: `None` only if the constant pattern stopped compiling, which the unit test below
// would catch first; every name is then refused, never accepted.
static IDENTIFIER_RE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(IDENTIFIER).ok());

/// Whether `name` is a CSDL simple identifier: 1 to 128 characters, the first `_` or a letter
/// (Unicode L, Nl), the rest `_`, letters, decimal digits (Nd), marks (Mn, Mc), connector
/// punctuation (Pc) or format characters (Cf); exactly `edm.xsd`'s `TSimpleIdentifier`.
///
/// ```
/// use odata_csdl::is_simple_identifier;
/// assert!(is_simple_identifier("_types"));
/// assert!(is_simple_identifier("Straße2"));
/// assert!(!is_simple_identifier("ngsi-ld:name"));
/// assert!(!is_simple_identifier("m²")); // `²` is Unicode No, not Nd
/// ```
#[must_use]
pub fn is_simple_identifier(name: &str) -> bool {
    if name.is_ascii() {
        // The same classes restricted to ASCII: L is `A-Za-z`, Nd `0-9`, Pc `_`; Nl, Mn, Mc and
        // Cf have no ASCII member. Most names are ASCII, and the automaton is slow to build.
        let mut bytes = name.bytes();
        return name.len() <= 128
            && bytes
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
    }
    name.chars().nth(128).is_none()
        && IDENTIFIER_RE
            .as_ref()
            .is_some_and(|pattern| pattern.is_match(name))
}

/// Whether `name` is a namespace: simple identifiers joined by `.`, at most 511 characters.
///
/// ```
/// assert!(odata_csdl::is_namespace("Org.OData.Core.V1"));
/// assert!(!odata_csdl::is_namespace("Org..Core"));
/// ```
#[must_use]
pub fn is_namespace(name: &str) -> bool {
    name.chars().nth(511).is_none() && name.split('.').all(is_simple_identifier)
}

/// `<namespace or alias>.<simple identifier>` split at its last `.`, when it is one.
pub(crate) fn split_qualified(name: &str) -> Option<(&str, &str)> {
    let (qualifier, simple) = name.rsplit_once('.')?;
    (is_namespace(qualifier) && is_simple_identifier(simple)).then_some((qualifier, simple))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identifier_pattern_compiles() {
        assert!(IDENTIFIER_RE.is_some());
    }

    /// The ASCII shortcut answers what the pattern answers, for every one- and two-character
    /// ASCII name.
    #[test]
    fn the_ascii_shortcut_is_the_pattern() {
        let pattern = IDENTIFIER_RE.as_ref().unwrap();
        let ascii = || (0..128_u8).map(char::from);
        for first in ascii() {
            for second in [
                None,
                Some('a'),
                Some('0'),
                Some('_'),
                Some('-'),
                Some(first),
            ] {
                let name: String = std::iter::once(first).chain(second).collect();
                assert_eq!(
                    is_simple_identifier(&name),
                    pattern.is_match(&name),
                    "{name:?}"
                );
            }
        }
        assert!(is_simple_identifier(&"a".repeat(128)));
        assert!(!is_simple_identifier(&"a".repeat(129)));
    }

    #[test]
    fn identifiers_follow_the_xsd_classes() {
        for good in [
            "a",
            "_",
            "A1",
            "x_y",
            "Ωmega",
            "ⅩⅡ",
            "a\u{0301}",
            "a\u{200B}",
            "a‿b",
        ] {
            assert!(is_simple_identifier(good), "{good}");
        }
        for bad in [
            "",
            "1a",
            "a-b",
            "a.b",
            "a b",
            "a:b",
            "\u{0301}a",
            "a²",
            "$x",
            "@x",
        ] {
            assert!(!is_simple_identifier(bad), "{bad}");
        }
    }

    #[test]
    fn identifiers_are_at_most_128_characters() {
        let at = "é".repeat(128);
        assert!(is_simple_identifier(&at));
        assert!(!is_simple_identifier(&format!("{at}é")));
    }

    #[test]
    fn namespaces_are_dotted_identifiers_up_to_511_characters() {
        assert!(is_namespace("a"));
        assert!(is_namespace("a.b.c"));
        assert!(!is_namespace(""));
        assert!(!is_namespace(".a"));
        assert!(!is_namespace("a."));
        let long = vec!["a".repeat(127); 4].join(".");
        assert_eq!(long.chars().count(), 511);
        assert!(is_namespace(&long));
        assert!(!is_namespace(&format!("{long}b")));
    }

    #[test]
    fn qualified_names_split_at_the_last_dot() {
        assert_eq!(
            split_qualified("Org.OData.Core.V1.Computed"),
            Some(("Org.OData.Core.V1", "Computed"))
        );
        assert_eq!(split_qualified("Core.Computed"), Some(("Core", "Computed")));
        assert_eq!(split_qualified("Computed"), None);
        assert_eq!(split_qualified("Core."), None);
        assert_eq!(split_qualified(".Computed"), None);
    }
}
