// SPDX-License-Identifier: EUPL-1.2
//! A value that must never reach a log line, an error message or a response
//! (T-3915): the `/q` admin token and the database URL with its password.
//! `Debug` and `Display` print nothing of it, and it is not `Serialize`, so a
//! `{:?}`, a `tracing` field or a state dump cannot carry it. The one way
//! out is [`Secret::expose`], and `dev/security.cypher` (`secret-exposed`)
//! lists every call with where the value goes.

/// A secret string; see the module documentation.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value read from the environment or a mounted file.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself, for the one call that needs it (a connect, a
    /// comparison). Never for a log line or an error.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn neither_debug_nor_display_prints_the_value() {
        let s = Secret::new("postgresql://antares:hunter2@db/antares");
        for shown in [format!("{s:?}"), format!("{s}"), format!("{:#?}", Some(&s))] {
            assert!(!shown.contains("hunter2"), "{shown}");
        }
        assert_eq!(s.expose(), "postgresql://antares:hunter2@db/antares");
    }
}
