#![forbid(unsafe_code)]
//! Strongly-typed UUIDv7 identifiers and cryptographic helpers (§5.3, R7, R12).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// Constant-time string comparison for access tokens and digests (R7, R12).
#[must_use]
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Lowercase hex SHA-256 digest of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

macro_rules! define_uuid_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
            JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Generates a new time-ordered UUIDv7 identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Deterministic identifier for tests/fixtures from a 128-bit integer.
            #[must_use]
            pub const fn from_u128(v: u128) -> Self {
                Self(Uuid::from_u128(v))
            }

            /// Returns the inner [`Uuid`].
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(u: Uuid) -> Self {
                Self(u)
            }
        }
    };
}

define_uuid_id!(
    InstallationId,
    "Installation identifier present on every row (§5.3)."
);
define_uuid_id!(
    OrgId,
    "Organization identifier (UUIDv7) enforced by Postgres forced RLS (§5.3)."
);
define_uuid_id!(
    InstanceId,
    "Daiya instance identifier within an organization (§5.3)."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_compare_and_sha256_work() {
        assert!(constant_time_eq("token-abc-123", "token-abc-123"));
        assert!(!constant_time_eq("token-abc-123", "token-abc-124"));
        assert!(!constant_time_eq("short", "longer-string"));

        let d1 = sha256_hex(b"daiya");
        let d2 = sha256_hex(b"daiya");
        let d3 = sha256_hex(b"other");
        assert_eq!(d1, d2);
        assert_ne!(d1, d3);
        assert_eq!(d1.len(), 64);
    }

    #[test]
    fn uuid_ids_round_trip() {
        let org = OrgId::new();
        let parsed: OrgId = org.to_string().parse().expect("valid uuid");
        assert_eq!(org, parsed);
    }
}
