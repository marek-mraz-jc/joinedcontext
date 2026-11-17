#![forbid(unsafe_code)]
//! The tenancy key of `jc` (ARCH/TEN-01, TEN-04, TEN-10).
//!
//! - [`OrgId`], [`ProjectId`], [`SpaceId`]: 128 random bits each. No `Default`, no `From<&str>`:
//!   an id is minted ([`ProjectId::mint`]) or parsed from verified input ([`ProjectId::parse_verified`],
//!   [`ProjectId::from_path_segment`]). Nothing derives one from a name, host or URN.
//! - [`ProjectKey`]: the pair (`OrgId`, `ProjectId`), the key of every carrier (row, transaction,
//!   subject, object, cache, log field, secret path).
//! - [`internal_name`]: the one function that builds an internal space, Tenant, subject, object,
//!   queue or bucket name. Its output holds the two ids and a checked [`Label`], nothing else, so a
//!   branch project with the same Git files as its parent still gets names of its own.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Why a value is not an id or a label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// Not a UUID, or the nil UUID.
    NotAnId(&'static str),
    /// Not 26 characters of lower-case base32.
    NotAPathSegment(&'static str),
    /// A label outside the slug grammar (ARCH/MCP-31).
    NotALabel,
    /// Not `{OrgId}__{ProjectId}__{label}` (ARCH/TEN-10).
    NotAnInternalName,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnId(kind) => write!(f, "{kind} must be a non-nil UUID"),
            Self::NotAPathSegment(kind) => {
                write!(f, "{kind} in a path must be 26 characters of a-z and 2-7")
            }
            Self::NotALabel => f.write_str(
                "a name must be lower-case letters, digits and hyphens, start with a letter and hold at most 63 characters, e.g. bikes-public",
            ),
            Self::NotAnInternalName => f.write_str(
                "an internal name is two 26-character ids and a name joined by __, e.g. {OrgId}__{ProjectId}__bikes",
            ),
        }
    }
}

impl std::error::Error for KeyError {}

const B32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
/// Length of an id in a path: 128 bits in 5-bit groups.
pub const PATH_SEGMENT_LEN: usize = 26;

fn to_b32(bytes: [u8; 16]) -> String {
    let n = u128::from_be_bytes(bytes);
    // 26 groups of 5 bits = 130 bits: the first group holds the top 3 bits.
    (0..PATH_SEGMENT_LEN)
        .rev()
        .map(|i| char::from(B32[((n >> (i * 5)) & 31) as usize]))
        .collect()
}

fn from_b32(text: &str) -> Option<[u8; 16]> {
    if text.len() != PATH_SEGMENT_LEN {
        return None;
    }
    let mut n: u128 = 0;
    for (i, c) in text.bytes().enumerate() {
        let v = B32.iter().position(|&b| b == c)? as u128;
        // The first character carries 3 bits; a larger value would overflow 128 bits.
        if i == 0 && v > 7 {
            return None;
        }
        n = (n << 5) | v;
    }
    Some(n.to_be_bytes())
}

macro_rules! id_type {
    ($name:ident, $kind:literal) => {
        #[doc = concat!("A ", $kind, " id: 128 random bits (ARCH/TEN-01).")]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "Uuid", into = "Uuid")]
        pub struct $name(Uuid);

        impl $name {
            /// A new random id.
            #[must_use]
            pub fn mint() -> Self {
                Self(Uuid::new_v4())
            }

            /// Parses a stored or verified UUID; the nil UUID is refused.
            pub fn parse_verified(text: &str) -> Result<Self, KeyError> {
                Uuid::parse_str(text)
                    .ok()
                    .and_then(|u| Self::try_from(u).ok())
                    .ok_or(KeyError::NotAnId($kind))
            }

            /// Parses the path form written by [`Self::path_segment`].
            pub fn from_path_segment(text: &str) -> Result<Self, KeyError> {
                from_b32(text)
                    .and_then(|b| Self::try_from(Uuid::from_bytes(b)).ok())
                    .ok_or(KeyError::NotAPathSegment($kind))
            }

            /// The id in a path or a name: 26 characters of lower-case base32 without padding.
            #[must_use]
            pub fn path_segment(&self) -> String {
                to_b32(self.0.into_bytes())
            }

            /// The id as a UUID, for a `uuid` column or a `set_config` value.
            #[must_use]
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl TryFrom<Uuid> for $name {
            type Error = KeyError;
            fn try_from(u: Uuid) -> Result<Self, KeyError> {
                if u.is_nil() {
                    Err(KeyError::NotAnId($kind))
                } else {
                    Ok(Self(u))
                }
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Uuid {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}

id_type!(OrgId, "Organization id");
id_type!(ProjectId, "project id");
id_type!(SpaceId, "Context Space id");

/// The tenancy key (ARCH/TEN-04): every carrier holds both ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProjectKey {
    pub org: OrgId,
    pub project: ProjectId,
}

impl ProjectKey {
    #[must_use]
    pub const fn new(org: OrgId, project: ProjectId) -> Self {
        Self { org, project }
    }

    /// Object store prefix `org/{OrgId}/project/{ProjectId}/` (ARCH/STO-14, SEC-02), ids in their
    /// path form (TEN-01): the dashed UUID text never stands in a path.
    #[must_use]
    pub fn object_prefix(&self) -> String {
        format!(
            "org/{}/project/{}/",
            self.org.path_segment(),
            self.project.path_segment()
        )
    }

    /// `{OrgId}-{ProjectId}`, 53 characters: the key in a carrier that forbids `_` (DNS label,
    /// Kubernetes object name, S3 user or bucket name). Such a carrier holds ids only, never a
    /// name, and parses by the fixed id positions (ARCH/TEN-04).
    #[must_use]
    pub fn dns_label(&self) -> String {
        format!(
            "{}-{}",
            self.org.path_segment(),
            self.project.path_segment()
        )
    }

    /// NATS subject `jc.{OrgId}.{ProjectId}.{module}.{event}` (ARCH/STO-15). Module and event
    /// are labels.
    #[must_use]
    pub fn subject(&self, module: &Label, event: &Label) -> String {
        format!(
            "jc.{}.{}.{}.{}",
            self.org.path_segment(),
            self.project.path_segment(),
            module.0,
            event.0
        )
    }
}

/// A checked name: lower-case ASCII letters, digits and hyphens, starts with a letter, at most
/// 63 characters (the slug grammar of ARCH/MCP-31). A label never carries a key on its own.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Label(String);

impl Label {
    pub fn parse(text: &str) -> Result<Self, KeyError> {
        let ok = (1..=63).contains(&text.len())
            && text.as_bytes()[0].is_ascii_lowercase()
            && text
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(KeyError::NotALabel)
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Label {
    type Error = KeyError;
    fn try_from(s: String) -> Result<Self, KeyError> {
        Self::parse(&s)
    }
}

impl From<Label> for String {
    fn from(l: Label) -> String {
        l.0
    }
}

/// An internal name built by [`internal_name`]; the only way to get one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InternalName(String);

impl InternalName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key and label of a name [`internal_name`] built; anything else is refused.
    pub fn parse(text: &str) -> Result<(ProjectKey, Label), KeyError> {
        let mut parts = text.splitn(3, INTERNAL_NAME_JOINER);
        let (Some(org), Some(project), Some(label)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(KeyError::NotAnInternalName);
        };
        let key = ProjectKey::new(
            OrgId::from_path_segment(org)?,
            ProjectId::from_path_segment(project)?,
        );
        Ok((key, Label::parse(label)?))
    }
}

impl fmt::Display for InternalName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The joiner of an internal name: no id (base32) and no label (slug grammar) holds `_`, so the
/// parts split back without ambiguity (ARCH/TEN-10).
pub const INTERNAL_NAME_JOINER: &str = "__";

/// Longest internal name: two path segments, two joiners and a label.
pub const INTERNAL_NAME_MAX: usize = 2 * PATH_SEGMENT_LEN + 2 * INTERNAL_NAME_JOINER.len() + 63;

/// The one builder of internal space, Tenant, queue and table-key names (ARCH/TEN-10):
/// `{OrgId}__{ProjectId}__{label}`, ids in their path form. Two projects with the same label never
/// share a name; the label is checked, so the name fits [`INTERNAL_NAME_MAX`] and
/// [`InternalName::parse`] gives back the key and the label.
#[must_use]
pub fn internal_name(key: ProjectKey, name: &Label) -> InternalName {
    InternalName(format!(
        "{}{INTERNAL_NAME_JOINER}{}{INTERNAL_NAME_JOINER}{}",
        key.org.path_segment(),
        key.project.path_segment(),
        name.0
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_segment_round_trips_and_has_fixed_length() {
        for _ in 0..1000 {
            let p = ProjectId::mint();
            let seg = p.path_segment();
            assert_eq!(seg.len(), PATH_SEGMENT_LEN);
            assert_eq!(ProjectId::from_path_segment(&seg), Ok(p));
        }
        let max =
            ProjectId::try_from(Uuid::from_u128(u128::MAX)).unwrap_or_else(|_| ProjectId::mint());
        assert_eq!(max.path_segment(), format!("h{}", "7".repeat(25)));
    }

    #[test]
    fn path_segment_refuses_overflow_wrong_length_and_foreign_characters() {
        let bad = [
            "",
            "abc",
            &"a".repeat(25),
            &"a".repeat(27),
            &format!("i{}", "a".repeat(25)),
            &format!("A{}", "a".repeat(25)),
            &format!("a{}1", "a".repeat(24)),
            &"a".repeat(26),
        ];
        for text in bad {
            assert!(ProjectId::from_path_segment(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn nil_and_garbage_are_not_ids() {
        assert!(OrgId::parse_verified("00000000-0000-0000-0000-000000000000").is_err());
        assert!(OrgId::parse_verified("sk_bbsk").is_err());
        assert!(
            serde_json::from_str::<ProjectId>("\"00000000-0000-0000-0000-000000000000\"").is_err()
        );
        let p = ProjectId::mint();
        assert_eq!(ProjectId::parse_verified(&p.to_string()), Ok(p));
    }

    #[test]
    fn label_grammar() {
        for ok in ["a", "bikes-public", "x1", &"a".repeat(63)] {
            assert!(Label::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "1a",
            "-a",
            "A",
            "a_b",
            "a.b",
            "a/b",
            "a b",
            "á",
            &"a".repeat(64),
        ] {
            assert_eq!(Label::parse(bad), Err(KeyError::NotALabel), "{bad}");
        }
    }

    #[test]
    fn same_label_in_a_branch_gives_a_different_name() {
        let org = OrgId::mint();
        let parent = ProjectKey::new(org, ProjectId::mint());
        let branch = ProjectKey::new(org, ProjectId::mint());
        let space = Label::parse("bikes").unwrap_or_else(|e| panic!("{e}"));
        let a = internal_name(parent, &space);
        let b = internal_name(branch, &space);
        assert_ne!(a, b);
        assert!(a.as_str().ends_with("__bikes"));
        assert!(a.as_str().len() <= INTERNAL_NAME_MAX);
        assert!(
            a.as_str()
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
        );
        assert_eq!(InternalName::parse(a.as_str()), Ok((parent, space.clone())));
        let other_org = ProjectKey::new(OrgId::mint(), parent.project);
        assert_ne!(internal_name(other_org, &space), a);
    }

    #[test]
    fn carriers_hold_both_ids() {
        let k = ProjectKey::new(OrgId::mint(), ProjectId::mint());
        let m = Label::parse("shako").unwrap_or_else(|e| panic!("{e}"));
        let e = Label::parse("applied").unwrap_or_else(|e| panic!("{e}"));
        let s = k.subject(&m, &e);
        assert_eq!(s.split('.').count(), 5);
        assert_eq!(s.split('.').nth(2), Some(k.project.path_segment().as_str()));
        assert!(k.object_prefix().contains(&k.project.path_segment()));
    }

    /// ARCH/TEN-01, TEN-04: every carrier of one key spells the ids the same way, in the same
    /// order; only the joiner follows the carrier, and the dashed UUID text is in none of them.
    #[test]
    fn every_carrier_spells_one_key_the_same_way() {
        let k = ProjectKey::new(OrgId::mint(), ProjectId::mint());
        let (o, p) = (k.org.path_segment(), k.project.path_segment());
        let m = Label::parse("shako").unwrap_or_else(|e| panic!("{e}"));
        let n = Label::parse("bikes").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(k.object_prefix(), format!("org/{o}/project/{p}/"));
        assert_eq!(k.subject(&m, &m), format!("jc.{o}.{p}.shako.shako"));
        assert_eq!(internal_name(k, &n).as_str(), format!("{o}__{p}__bikes"));
        assert_eq!(k.dns_label(), format!("{o}-{p}"));
        assert!(k.dns_label().len() <= 63);
        for carrier in [
            k.object_prefix(),
            k.subject(&m, &m),
            internal_name(k, &n).0,
            k.dns_label(),
        ] {
            assert!(!carrier.contains(&k.org.to_string()), "{carrier}");
            assert!(!carrier.contains(&k.project.to_string()), "{carrier}");
        }
    }

    #[test]
    fn an_internal_name_parses_back_and_nothing_else_does() {
        let k = ProjectKey::new(OrgId::mint(), ProjectId::mint());
        let n = Label::parse("air-quality").unwrap_or_else(|e| panic!("{e}"));
        let name = internal_name(k, &n);
        assert_eq!(InternalName::parse(name.as_str()), Ok((k, n.clone())));
        let (o, p) = (k.org.path_segment(), k.project.path_segment());
        for bad in [
            format!("{o}-{p}-bikes"),
            format!("{o}__{p}"),
            format!("{o}__{p}__bi__kes"),
            format!("{o}__{p}__Bikes"),
            format!("{}__{p}__bikes", k.org),
            format!("__{p}__bikes"),
        ] {
            assert!(InternalName::parse(&bad).is_err(), "{bad}");
        }
    }
}
