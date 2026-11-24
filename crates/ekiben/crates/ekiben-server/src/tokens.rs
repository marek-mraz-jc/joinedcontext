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

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).expect("write");
    }

    #[test]
    fn lines_parse_and_errors_name_the_line_not_its_content() {
        let text = format!(
            "# comment\n\n  org-a {DIGEST}  \n{}\n",
            token_line("org-b", "t")
        );
        let entries = parse_token_lines(&text).expect("valid");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, OrgId::new("org-a"));
        assert_eq!(entries[1].1.as_slice(), Sha256::digest(b"t").as_slice());
        for (bad, why) in [
            ("org-a".to_string(), "line 1: expected"),
            (format!("org-a {DIGEST} extra"), "line 1: expected"),
            (
                format!("# c\nbad/org {DIGEST}"),
                "line 2: invalid organization id",
            ),
            (
                "org-a not-hex-secret".to_string(),
                "line 1: the digest is not 64 hex digits",
            ),
            (
                "org-a abcd".to_string(),
                "line 1: the digest is not 64 hex digits",
            ),
        ] {
            let err = parse_token_lines(&bad).expect_err(&bad);
            assert!(err.starts_with(why), "{bad}: {err}");
            assert!(!err.contains("secret"), "{err}");
        }
    }

    #[test]
    fn more_than_the_entry_limit_is_refused() {
        let text: String = (0..=MAX_ENTRIES)
            .map(|i| format!("org-{i} {DIGEST}\n"))
            .collect();
        let err = parse_token_lines(&text).expect_err("too many");
        assert_eq!(err, "more than 10000 tokens");
    }

    #[test]
    fn load_refuses_a_missing_oversized_or_malformed_file() {
        let dir = tempfile::tempdir().expect("dir");
        let missing = dir.path().join("gone");
        assert!(OrgTokenFile::load(&missing)
            .expect_err("missing")
            .contains("gone"));
        let big = dir.path().join("big");
        write(&big, &"#".repeat(MAX_FILE_BYTES as usize + 1));
        let err = OrgTokenFile::load(&big).expect_err("big");
        assert!(err.contains("larger than 1048576 bytes"), "{err}");
        let err = OrgTokenFile::load(dir.path()).expect_err("a directory");
        assert!(err.starts_with("org tokens file '"), "{err}");
        let bad = dir.path().join("bad");
        write(&bad, "org-a\n");
        assert!(OrgTokenFile::load(&bad)
            .expect_err("bad")
            .contains("line 1"));
    }

    #[test]
    fn tokens_match_their_organization_and_debug_shows_no_digest() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("tokens");
        write(
            &path,
            &format!(
                "{}\n{}\n",
                token_line("org-a", "ta"),
                token_line("org-b", "tb")
            ),
        );
        let file = OrgTokenFile::load(&path).expect("load");
        assert_eq!(file.path(), path);
        assert_eq!(file.len(), 2);
        assert!(!file.is_empty());
        assert_eq!(file.match_token(b"ta"), Some(OrgId::new("org-a")));
        assert_eq!(file.match_token(b"tb"), Some(OrgId::new("org-b")));
        assert_eq!(file.match_token(b"tc"), None);
        let debug = format!("{file:?}");
        assert!(debug.contains("entries: 2"), "{debug}");
        assert!(!debug.contains(&token_line("org-a", "ta")[6..]), "{debug}");
    }

    #[test]
    fn the_same_digest_twice_answers_the_first_organization() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("tokens");
        write(
            &path,
            &format!(
                "{}\n{}\n",
                token_line("org-a", "t"),
                token_line("org-b", "t")
            ),
        );
        let file = OrgTokenFile::load(&path).expect("load");
        assert_eq!(file.match_token(b"t"), Some(OrgId::new("org-a")));
    }

    #[test]
    fn reload_takes_a_new_file_keeps_tokens_on_a_broken_one_and_revokes_on_removal() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("tokens");
        write(&path, &token_line("org-a", "ta"));
        let file = OrgTokenFile::load(&path).expect("load");

        write(
            &path,
            &format!(
                "{}\n{}\n",
                token_line("org-a", "ta"),
                token_line("org-c", "tc")
            ),
        );
        // Within the recheck period a lookup does not read the file again.
        assert_eq!(file.match_token(b"tc"), None);
        file.reload();
        assert_eq!(file.match_token(b"tc"), Some(OrgId::new("org-c")));
        // An unchanged file is not read again.
        file.reload();
        assert_eq!(file.len(), 2);

        write(&path, "broken line with three\n");
        file.reload();
        assert_eq!(file.len(), 2, "a broken file keeps the previous tokens");

        std::fs::remove_file(&path).expect("rm");
        file.reload();
        assert!(file.is_empty(), "a removed file revokes every token");
        assert_eq!(file.match_token(b"ta"), None);
    }

    #[test]
    fn a_lookup_after_the_recheck_period_reads_a_changed_file() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("tokens");
        write(&path, &token_line("org-a", "ta"));
        let file = OrgTokenFile::load(&path).expect("load");
        write(&path, &format!("{}\n", token_line("org-d", "td")));
        std::thread::sleep(RECHECK_EVERY + Duration::from_millis(50));
        assert_eq!(file.match_token(b"td"), Some(OrgId::new("org-d")));
    }
}
