#![forbid(unsafe_code)]
//! One secret type for `jc` and the modules it hosts, the reader that turns `NAME` / `NAME_FILE`
//! into one, and the one helper that starts child processes.
//!
//! - [`Secret`] prints, formats and serializes as `<redacted>` and wipes its bytes on drop.
//!   [`Secret::expose`] is the only way to the value.
//! - [`read`] takes the two places a secret may come from: the `NAME` variable's value and a
//!   file (`NAME_FILE`, or the TOML key that names a file). Both set is refused, so an operator
//!   never wonders which one won. An error names the variable and the path, never the value.
//! - [`spawn::command`] builds a [`std::process::Command`] with a cleared environment and only
//!   [`spawn::PASSED`] carried over, so a child (git, a build) never inherits a module's secret.

use std::fmt;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

pub mod spawn;

/// A secret value. `Debug`, `Display` and `Serialize` show `<redacted>`; equality runs in time
/// that does not depend on where two values first differ; the bytes are zeroed on drop.
#[derive(Clone, Default)]
pub struct Secret(Zeroizing<String>);

/// What every formatting of a [`Secret`] prints.
pub const REDACTED: &str = "<redacted>";

impl Secret {
    /// Wraps `value`; the `String` is moved, so no copy stays behind.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// `bytes` random bytes as lower-case hex (`2 * bytes` characters).
    pub fn generate(bytes: usize) -> Result<Self, String> {
        let mut buf = Zeroizing::new(vec![0u8; bytes]);
        getrandom::fill(&mut buf).map_err(|e| format!("no randomness: {e}"))?;
        Ok(Self::new(hex::encode(&*buf)))
    }

    /// The value. Call it where the value leaves for its one use (a header, a key parser).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether `other` equals the value, compared without an early exit.
    #[must_use]
    pub fn matches(&self, other: &[u8]) -> bool {
        constant_time_eq(self.0.as_bytes(), other)
    }
}

/// Compares two byte strings in time that depends only on their lengths.
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl PartialEq for Secret {
    fn eq(&self, other: &Self) -> bool {
        self.matches(other.0.as_bytes())
    }
}

impl Eq for Secret {}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl serde::Serialize for Secret {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(REDACTED)
    }
}

/// Where a secret came from, for the start report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The `NAME` variable (visible in `docker inspect` and the pod spec).
    Env,
    /// A file (`NAME_FILE` or the config file's key).
    File,
    /// Made by `jc` at start.
    Generated,
}

impl Source {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Env => "env",
            Source::File => "file",
            Source::Generated => "generated",
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a secret could not be read. No variant carries a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// `NAME` and `NAME_FILE` (or the config file's key) are both set.
    Both { name: String },
    /// The variable is set and empty.
    EmptyVar { name: String },
    /// The file is unreadable.
    File {
        name: String,
        path: PathBuf,
        reason: String,
    },
    /// The file holds only whitespace.
    EmptyFile { name: String, path: PathBuf },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Both { name } => write!(
                f,
                "{name} and {name}_FILE are both set: give the secret once, preferably as a file"
            ),
            ReadError::EmptyVar { name } => write!(f, "{name} is set and empty"),
            ReadError::File { name, path, reason } => {
                write!(f, "{name}_FILE {}: unreadable ({reason})", path.display())
            }
            ReadError::EmptyFile { name, path } => {
                write!(f, "{name}_FILE {} is empty", path.display())
            }
        }
    }
}

impl std::error::Error for ReadError {}

/// Reads a secret file: the whole file, trimmed; an empty file is refused. A file that is not
/// UTF-8 (a raw binary key) is held as the lower-case hex of its bytes, untrimmed, which every
/// key parser `jc` hands it to reads. Errors name `name` and the path only.
pub fn read_file(name: &str, path: &Path) -> Result<Secret, ReadError> {
    let raw = Zeroizing::new(std::fs::read(path).map_err(|e| ReadError::File {
        name: name.to_owned(),
        path: path.to_path_buf(),
        reason: e.kind().to_string(),
    })?);
    let Ok(text) = std::str::from_utf8(&raw) else {
        return Ok(Secret::new(hex::encode(&*raw)));
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ReadError::EmptyFile {
            name: name.to_owned(),
            path: path.to_path_buf(),
        });
    }
    Ok(Secret::new(trimmed.to_owned()))
}

/// The secret `name` from its value (`NAME`) or its file (`NAME_FILE`), with where it came from.
/// Neither set is `Ok(None)`; both set is refused.
pub fn read(
    name: &str,
    value: Option<String>,
    file: Option<&Path>,
) -> Result<Option<(Secret, Source)>, ReadError> {
    let value = value.map(Secret::new);
    match (value, file) {
        (Some(_), Some(_)) => Err(ReadError::Both {
            name: name.to_owned(),
        }),
        (Some(v), None) => {
            if v.expose().trim().is_empty() {
                return Err(ReadError::EmptyVar {
                    name: name.to_owned(),
                });
            }
            let trimmed = v.expose().trim().to_owned();
            Ok(Some((Secret::new(trimmed), Source::Env)))
        }
        (None, Some(path)) => read_file(name, path).map(|s| Some((s, Source::File))),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALUE: &str = "s3cret-value-0123456789";

    #[test]
    fn every_formatting_is_redacted() {
        let s = Secret::new(VALUE.to_owned());
        assert_eq!(format!("{s:?}"), REDACTED);
        assert_eq!(format!("{s}"), REDACTED);
        assert_eq!(format!("{:?}", Some(&s)), format!("Some({REDACTED})"));
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            format!("\"{REDACTED}\"")
        );
        assert_eq!(s.expose(), VALUE);
    }

    #[test]
    fn equality_is_by_value() {
        let a = Secret::new(VALUE.to_owned());
        assert_eq!(a, Secret::new(VALUE.to_owned()));
        assert_ne!(a, Secret::new("other".to_owned()));
        assert!(a.matches(VALUE.as_bytes()));
        assert!(!a.matches(b"s3cret"));
    }

    #[test]
    fn generated_secrets_are_hex_and_differ() {
        let a = Secret::generate(32).unwrap();
        let b = Secret::generate(32).unwrap();
        assert_eq!(a.expose().len(), 64);
        assert!(a.expose().bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn name_and_name_file_both_set_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tok");
        std::fs::write(&file, VALUE).unwrap();
        let e = read("JCBIN_X", Some(VALUE.into()), Some(&file)).unwrap_err();
        assert_eq!(
            e,
            ReadError::Both {
                name: "JCBIN_X".into()
            }
        );
        assert!(!e.to_string().contains(VALUE));
        let (s, src) = read("JCBIN_X", None, Some(&file)).unwrap().unwrap();
        assert_eq!((s.expose(), src), (VALUE, Source::File));
        let (s, src) = read("JCBIN_X", Some(format!(" {VALUE}\n")), None)
            .unwrap()
            .unwrap();
        assert_eq!((s.expose(), src), (VALUE, Source::Env));
        assert_eq!(read("JCBIN_X", None, None).unwrap(), None);
    }

    #[test]
    fn a_binary_key_file_is_held_as_hex() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("kek");
        let raw: Vec<u8> = (0..32u8).map(|b| b.wrapping_mul(37) | 0x80).collect();
        std::fs::write(&file, &raw).unwrap();
        assert_eq!(read_file("K", &file).unwrap().expose(), hex::encode(&raw));
    }

    #[test]
    fn errors_name_the_variable_and_path_never_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        std::fs::write(&empty, " \n").unwrap();
        let e = read("JCBIN_X", None, Some(&empty)).unwrap_err().to_string();
        assert!(e.contains("JCBIN_X_FILE") && e.contains("empty"), "{e}");
        let e = read("JCBIN_X", None, Some(&dir.path().join("missing")))
            .unwrap_err()
            .to_string();
        assert!(e.contains("missing"), "{e}");
        let e = read("JCBIN_X", Some("  ".into()), None).unwrap_err();
        assert_eq!(
            e,
            ReadError::EmptyVar {
                name: "JCBIN_X".into()
            }
        );
    }
}
