//! `--org-tokens-file`: the organization tokens of a hosted server, as `<org> <sha256 hex>` lines
//! (`#` starts a comment). Only digests are stored, so the file holds no usable token. A
//! presented token is hashed and compared with every entry in constant time. The file is read
//! again when its modification time or size changes (checked at most once a second); a file that
//! no longer parses keeps the previous tokens, a file that is gone revokes them all.

use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime};

use ekiben_core::OrgId;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const RECHECK_EVERY: Duration = Duration::from_secs(1);
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

type Stamp = Option<(SystemTime, u64)>;

struct TokenState {
    entries: Vec<(OrgId, [u8; 32])>,
    stamp: Stamp,
    checked_at: Instant,
}

/// The organization tokens of `--org-tokens-file`.
pub struct OrgTokenFile {
    path: PathBuf,
    state: RwLock<TokenState>,
}

impl std::fmt::Debug for OrgTokenFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrgTokenFile")
            .field("path", &self.path)
            .field("entries", &self.len())
            .finish()
    }
}

fn stamp_of(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn valid_org(org: &str) -> bool {
    !org.is_empty()
        && org.len() <= 256
        && org
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
}

/// Parses the file text; an error names the line, never its content.
pub fn parse_token_lines(text: &str) -> Result<Vec<(OrgId, [u8; 32])>, String> {
    let mut entries = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(org), Some(digest), None) = (fields.next(), fields.next(), fields.next()) else {
            return Err(format!("line {}: expected `<org> <sha256 hex>`", i + 1));
        };
        if !valid_org(org) {
            return Err(format!("line {}: invalid organization id", i + 1));
        }
        let bytes = hex::decode(digest)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .ok_or_else(|| format!("line {}: the digest is not 64 hex digits", i + 1))?;
        if entries.len() >= MAX_ENTRIES {
            return Err(format!("more than {MAX_ENTRIES} tokens"));
        }
        entries.push((OrgId::new(org), bytes));
    }
    Ok(entries)
}

fn read_entries(path: &Path) -> Result<Vec<(OrgId, [u8; 32])>, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("larger than {MAX_FILE_BYTES} bytes"));
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_token_lines(&text)
}

/// The line to put in the file for `token` of `org`.
pub fn token_line(org: &str, token: &str) -> String {
    format!("{org} {}", hex::encode(Sha256::digest(token.as_bytes())))
}

impl OrgTokenFile {
    /// Reads the file; it must exist and parse at start.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let entries = read_entries(&path)
            .map_err(|e| format!("org tokens file '{}': {e}", path.display()))?;
        let stamp = stamp_of(&path);
        Ok(Self {
            path,
            state: RwLock::new(TokenState {
                entries,
                stamp,
                checked_at: Instant::now(),
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How many tokens are loaded.
    pub fn len(&self) -> usize {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn refresh(&self, force: bool) {
        {
            let st = self.state.read().unwrap_or_else(PoisonError::into_inner);
            if !force && st.checked_at.elapsed() < RECHECK_EVERY {
                return;
            }
        }
        let stamp = stamp_of(&self.path);
        let mut st = self.state.write().unwrap_or_else(PoisonError::into_inner);
        st.checked_at = Instant::now();
        if stamp == st.stamp {
            return;
        }
        if stamp.is_none() {
            tracing::warn!(path = %self.path.display(), "org tokens file is gone: every organization token is revoked");
            st.entries.clear();
            st.stamp = None;
            return;
        }
        match read_entries(&self.path) {
            Ok(entries) => {
                st.entries = entries;
                st.stamp = stamp;
            }
            Err(e) => {
                tracing::error!(path = %self.path.display(), "org tokens file: {e}; the previous tokens stay");
                st.stamp = stamp;
            }
        }
    }

    /// Reads the file again now if it changed.
    pub fn reload(&self) {
        self.refresh(true);
    }

    /// The organization whose token `provided` is.
    pub fn match_token(&self, provided: &[u8]) -> Option<OrgId> {
        self.refresh(false);
        let digest = Sha256::digest(provided);
        let st = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let mut found: Option<OrgId> = None;
        for (org, want) in &st.entries {
            let same: bool = digest.as_slice().ct_eq(want.as_slice()).into();
            if same && found.is_none() {
                found = Some(org.clone());
            }
        }
        found
    }
}
