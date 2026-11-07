// SPDX-License-Identifier: EUPL-1.2
//! The three ways a load fails (ADR-0024 decisions 4 and 5).

use std::fmt;

/// Why a LinkML document was not loaded.
///
/// ```
/// use linkml_model::{Error, Limits, Model};
///
/// let limits = Limits { max_bytes: 8, max_depth: 8, max_elements: 8 };
/// let err = Model::read(br#"{"id": "https://example.org/s", "name": "s"}"#, &limits).unwrap_err();
/// assert_eq!(err, Error::LimitExceeded { limit: "max_bytes", max: 8 });
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Not a LinkML schema: invalid JSON, a wrong type, a dangling reference,
    /// a name with a control character, an IRI that is not one.
    Malformed(String),
    /// Valid LinkML outside the subset this crate implements: a metaslot it
    /// does not know, a range that is not a class, an enum or a built-in type.
    /// Refused, never ignored (threat LK4, ADR-0023 rule 10).
    NotImplemented(String),
    /// A bound of the caller's [`Limits`](crate::Limits) was reached.
    LimitExceeded {
        /// The field of [`Limits`](crate::Limits) that was reached.
        limit: &'static str,
        /// Its value.
        max: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "malformed LinkML schema: {why}"),
            Self::NotImplemented(what) => write!(f, "outside the LinkML subset: {what}"),
            Self::LimitExceeded { limit, max } => {
                write!(f, "LinkML schema exceeds {limit} ({max})")
            }
        }
    }
}

impl std::error::Error for Error {}
