#![forbid(unsafe_code)]
//! Strongly-typed UUIDv7 identifiers and constant-time cryptographic helpers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// Constant-time byte comparison.
#[must_use]
pub fn constant_time_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

/// Constant-time string comparison for tokens, digests, and HMACs.
#[must_use]
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    constant_time_eq_bytes(a.as_bytes(), b.as_bytes())
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

            /// Deterministic identifier from a 128-bit integer.
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

define_uuid_id!(InstanceId, "Forge instance identifier present on every row (S2).");
define_uuid_id!(OrgId, "Organization identifier enforced by Postgres forced RLS (S2).");
define_uuid_id!(RepoId, "Repository identifier used as the sole filesystem key (S2, T1).");
define_uuid_id!(WebhookId, "Webhook identifier.");
define_uuid_id!(PackageId, "Package identifier.");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_constant_time_helpers_work() {
        let id = RepoId::new();
        let parsed: RepoId = id.to_string().parse().expect("valid uuid");
        assert_eq!(id, parsed);
        assert!(constant_time_eq("abc123", "abc123"));
        assert!(!constant_time_eq("abc123", "abc124"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert_eq!(sha256_hex(b"shako").len(), 64);
    }
}
