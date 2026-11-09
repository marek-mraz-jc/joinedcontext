// SPDX-License-Identifier: MIT
//! [`GitTable`]: the YAML and JSON files of a repository at one commit, one file one row.
//!
//! The files are read from the object store (`git ls-tree`, `git cat-file`), never from a
//! working tree, so a read cannot leave the repository: a path pattern refuses `..`, `.` and an
//! absolute path, and a symbolic link that matches the pattern refuses the whole set, wherever
//! it points. `git` runs with its arguments as an argument vector, the commit checked as a full
//! hexadecimal object name first, so no request value reaches a command line.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::{Fetched, Fetching, MemoryTable, Plan, Scope, Table, TableError, TableInfo};

/// How much of a repository one set reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitLimits {
    /// Files matching the pattern.
    pub max_files: usize,
    /// Bytes of one file.
    pub max_file_bytes: usize,
    /// Bytes of every file together.
    pub max_total_bytes: usize,
}

impl GitLimits {
    /// 10 000 files, 1 MiB each, 64 MiB together.
    #[must_use]
    pub fn strict() -> Self {
        Self {
            max_files: 10_000,
            max_file_bytes: 1 << 20,
            max_total_bytes: 64 << 20,
        }
    }
}

/// An entity set over the files of a Git repository at one commit that match a path pattern:
/// each `.yaml`, `.yml` or `.json` file is one row (an object holding the key property). The
/// commit is the set's entity tag.
#[derive(Debug, Clone)]
pub struct GitTable {
    rows: MemoryTable,
    commit: String,
}

/// A path pattern's segments: plain names and `*` wildcards within one segment.
fn segments(pattern: &str) -> Result<Vec<&str>, TableError> {
    let refused = || {
        TableError::BadRequest(format!(
            "the path pattern {pattern} leaves the repository root"
        ))
    };
    if pattern.is_empty()
        || pattern.starts_with('/')
        || pattern.contains('\\')
        || pattern.contains('\0')
    {
        return Err(refused());
    }
    let parts: Vec<&str> = pattern.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return Err(refused());
    }
    Ok(parts)
}

/// Whether `name` matches the glob segment `glob` (`*` any run of characters).
fn glob(glob: &str, name: &str) -> bool {
    let mut parts = glob.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let mut parts: Vec<&str> = parts.collect();
    let Some(last) = parts.pop() else {
        return rest.is_empty();
    };
    for part in parts {
        match rest.find(part) {
            Some(at) => rest = rest.get(at.saturating_add(part.len())..).unwrap_or(""),
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

fn matched(pattern: &[&str], path: &str) -> bool {
    let names: Vec<&str> = path.split('/').collect();
    names.len() == pattern.len() && pattern.iter().zip(&names).all(|(g, n)| glob(g, n))
}

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>, TableError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .output()
        .map_err(|e| TableError::Source(format!("git did not run: {e}")))?;
    if !out.status.success() {
        return Err(TableError::Source(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

impl GitTable {
    /// The set `info` over the files of `repo` at `commit` (a full 40 or 64 digit object name)
    /// whose repository path matches `pattern` (`spaces/*/endpoints/*.yaml`).
    ///
    /// # Errors
    ///
    /// [`TableError::BadRequest`] for a pattern leaving the root, a commit that is not a full
    /// object name, a matching symbolic link, a file over `limits`, or a file that is not an
    /// object with a unique key; [`TableError::Source`] when `git` fails.
    pub fn open(
        info: TableInfo,
        repo: impl Into<PathBuf>,
        commit: &str,
        pattern: &str,
        limits: GitLimits,
    ) -> Result<Self, TableError> {
        let repo = repo.into();
        let pattern = segments(pattern)?;
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(TableError::BadRequest(
                "the commit must be a full hexadecimal object name".into(),
            ));
        }
        let listing = git(
            &repo,
            &[
                "ls-tree",
                "-r",
                "-z",
                "--full-tree",
                "--end-of-options",
                commit,
            ],
        )?;
        let mut rows = Vec::new();
        let mut total = 0usize;
        for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let entry = String::from_utf8_lossy(entry);
            // "<mode> <type> <object>\t<path>"
            let Some((head, path)) = entry.split_once('\t') else {
                return Err(TableError::Source(
                    "git ls-tree wrote an entry without a path".into(),
                ));
            };
            if !matched(&pattern, path) {
                continue;
            }
            let mut fields = head.split(' ');
            let (mode, kind, object) = (fields.next(), fields.next(), fields.next().unwrap_or(""));
            if mode == Some("120000") {
                return Err(TableError::BadRequest(format!(
                    "{path} is a symbolic link; a set reads files only"
                )));
            }
            if kind != Some("blob") {
                continue;
            }
            if rows.len() >= limits.max_files {
                return Err(TableError::BadRequest(format!(
                    "more than {} files match",
                    limits.max_files
                )));
            }
            let size: usize = String::from_utf8_lossy(&git(&repo, &["cat-file", "-s", object])?)
                .trim()
                .parse()
                .map_err(|_| TableError::Source("git cat-file wrote no size".into()))?;
            total = total.saturating_add(size);
            if size > limits.max_file_bytes || total > limits.max_total_bytes {
                return Err(TableError::BadRequest(format!(
                    "{path} is over the size this set reads"
                )));
            }
            let bytes = git(&repo, &["cat-file", "blob", object])?;
            rows.push(parse(path, &bytes)?);
        }
        Ok(Self {
            rows: MemoryTable::new(info, rows)?,
            commit: commit.to_ascii_lowercase(),
        })
    }

    /// The commit the set reads.
    #[must_use]
    pub fn commit(&self) -> &str {
        &self.commit
    }
}

/// One file as a row: YAML for `.yaml` and `.yml`, JSON for `.json`.
fn parse(path: &str, bytes: &[u8]) -> Result<Value, TableError> {
    let bad = |why: String| TableError::BadRequest(format!("{path}: {why}"));
    let extension = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let value: Value = if extension.as_deref() == Some("json") {
        serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?
    } else if matches!(extension.as_deref(), Some("yaml" | "yml")) {
        serde_norway::from_slice(bytes).map_err(|e| bad(e.to_string()))?
    } else {
        return Err(bad("only .yaml, .yml and .json files are rows".into()));
    };
    if value.is_object() {
        Ok(value)
    } else {
        Err(bad("a row is an object".into()))
    }
}

impl Table for GitTable {
    fn info(&self) -> &TableInfo {
        self.rows.info()
    }

    fn fetch<'a>(&'a self, plan: &'a Plan, scope: &'a Scope) -> Fetching<'a> {
        Box::pin(std::future::ready(Ok::<Fetched, TableError>(
            self.rows.answer(plan, scope),
        )))
    }

    fn etag(&self) -> Option<&str> {
        Some(&self.commit)
    }
}

#[cfg(test)]
mod tests {
    use super::{glob, matched, segments};

    #[test]
    fn a_pattern_stays_under_the_root() {
        for bad in ["../x/*.yaml", "a/../b", "/etc/*", "a//b", "./a", "a\\b", ""] {
            assert!(segments(bad).is_err(), "{bad} was accepted");
        }
        assert!(segments("spaces/*/endpoints/*.yaml").is_ok());
    }

    #[test]
    fn a_glob_matches_one_segment() {
        assert!(glob("*.yaml", "a.yaml"));
        assert!(glob("a*b*c", "aXbYc"));
        assert!(!glob("*.yaml", "a.json"));
        assert!(!glob("a*a", "a"));
        assert!(matched(&["spaces", "*", "e.yaml"], "spaces/s1/e.yaml"));
        assert!(!matched(&["spaces", "*", "e.yaml"], "spaces/s1/x/e.yaml"));
    }
}
