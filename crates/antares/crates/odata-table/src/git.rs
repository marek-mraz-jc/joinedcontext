// SPDX-License-Identifier: MIT
//! [`GitTable`]: the YAML and JSON files of a repository at one commit, one file one row.
//!
//! The files are read from the object store (`git ls-tree`, `git cat-file`), never from a
//! working tree, so a read cannot leave the repository: a path pattern refuses `..`, `.` and an
//! absolute path, and a symbolic link that matches the pattern refuses the whole set, wherever
//! it points. `git` runs with its arguments as an argument vector, the commit checked as a full
//! hexadecimal object name first, so no request value reaches a command line.
//!
//! Decided (worker-t6659, owner may override): a write is a commit made with plumbing (a
//! temporary index, never a working tree) whose parent is the commit the set reads, and the
//! set's branch moves to it by compare-and-swap, so a branch someone else moved refuses the
//! write (409) instead of losing their commit. A set opened without a branch stays read only. A
//! module whose changes need review points the set at a proposal branch and lets the forge
//! merge it; options were a direct commit to the default branch, or no Git writes at all.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{PoisonError, RwLock};

use serde_json::Value;

use crate::memory::{answer, keyed};
use crate::write::{applied, located};
use crate::{
    Applying, Batch, Change, Fetched, Fetching, Plan, Scope, Table, TableError, TableInfo,
};

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
/// commit is the set's entity tag. Opened on a branch ([`GitTable::on_branch`]) it writes too:
/// each write is one commit on that branch, and the set then reads that commit.
#[derive(Debug)]
pub struct GitTable {
    info: TableInfo,
    repo: PathBuf,
    state: RwLock<Snapshot>,
    writer: Option<Writer>,
}

/// The rows of one commit, each with the file it came from.
#[derive(Debug)]
struct Snapshot {
    commit: String,
    rows: Vec<Value>,
    /// `(path, mode)` of each row, in row order.
    files: Vec<(String, String)>,
}

/// Where and as whom a writable set commits.
#[derive(Debug)]
struct Writer {
    branch: String,
    name: String,
    email: String,
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
    git_with(repo, args, &[], None)
}

/// `git` in `repo` with `env` set and `input` on its standard input.
fn git_with(
    repo: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    input: Option<&[u8]>,
) -> Result<Vec<u8>, TableError> {
    let out = spawned(repo, args, env, input)
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

/// Runs `git` to its end: spawned, `input` written, its output collected.
fn spawned(
    repo: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    input: Option<&[u8]>,
) -> std::io::Result<std::process::Output> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .envs(env.iter().copied())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(bytes)?;
    }
    child.wait_with_output()
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
        let mut files = Vec::new();
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
            // a size git cannot state reads as too large: the file is refused, never read unbounded
            let size: usize = String::from_utf8_lossy(&git(&repo, &["cat-file", "-s", object])?)
                .trim()
                .parse()
                .unwrap_or(usize::MAX);
            total = total.saturating_add(size);
            if size > limits.max_file_bytes || total > limits.max_total_bytes {
                return Err(TableError::BadRequest(format!(
                    "{path} is over the size this set reads"
                )));
            }
            let bytes = git(&repo, &["cat-file", "blob", object])?;
            rows.push(parse(path, &bytes)?);
            files.push((path.to_owned(), mode.unwrap_or("100644").to_owned()));
        }
        keyed(&info, &rows)?;
        Ok(Self {
            info,
            repo,
            state: RwLock::new(Snapshot {
                commit: commit.to_ascii_lowercase(),
                rows,
                files,
            }),
            writer: None,
        })
    }

    /// This set writing to `branch`, whose head must be the commit the set reads; each commit
    /// is authored and committed as `name <email>`.
    ///
    /// # Errors
    ///
    /// [`TableError::BadRequest`] for a branch name Git refuses, or a branch whose head is not
    /// the commit the set reads; [`TableError::Source`] when `git` fails.
    pub fn on_branch(mut self, branch: &str, name: &str, email: &str) -> Result<Self, TableError> {
        let refused = |why: &str| TableError::BadRequest(format!("the branch {branch} {why}"));
        if branch.starts_with('-')
            || name.contains(['<', '>', '\n'])
            || email.contains(['<', '>', '\n'])
        {
            return Err(refused("or its author is not a plain name"));
        }
        let reference = format!("refs/heads/{branch}");
        git(&self.repo, &["check-ref-format", &reference])
            .map_err(|_| refused("is not a valid branch name"))?;
        let head = git(
            &self.repo,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{reference}^{{commit}}"),
            ],
        )
        .map_err(|_| refused("does not exist"))?;
        if String::from_utf8_lossy(&head).trim() != self.commit() {
            return Err(refused("has moved past the commit this set reads"));
        }
        self.writer = Some(Writer {
            branch: branch.to_owned(),
            name: name.to_owned(),
            email: email.to_owned(),
        });
        Ok(self)
    }

    /// The commit the set reads.
    #[must_use]
    pub fn commit(&self) -> String {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .commit
            .clone()
    }

    /// Applies `batch` as one commit on the branch, then reads that commit.
    fn commit_batch(&self, writer: &Writer, batch: &Batch) -> Result<(), TableError> {
        // the write lock is held from the guard check to the new snapshot: one write at a time
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let at = located(&state.rows, &self.info.key, batch)?;
        let mut rows = state.rows.clone();
        applied(&mut rows, &at, &batch.change);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let index =
            std::env::temp_dir().join(format!("odata-table-index-{}-{stamp}", std::process::id()));
        let index_text = index.to_string_lossy().into_owned();
        let committed = self.committed(writer, &state, &at, &batch.change, &index_text);
        // the temporary index is gone whatever happened
        let _ = std::fs::remove_file(&index);
        let commit = committed?;
        let mut files = state.files.clone();
        if batch.change == Change::Delete {
            let mut i = 0usize;
            files.retain(|_| {
                let keep = !at.contains(&i);
                i = i.saturating_add(1);
                keep
            });
        }
        *state = Snapshot {
            commit,
            rows,
            files,
        };
        Ok(())
    }

    /// Writes the commit and moves the branch to it, answering the new commit.
    fn committed(
        &self,
        writer: &Writer,
        state: &Snapshot,
        at: &[usize],
        change: &Change,
        index: &str,
    ) -> Result<String, TableError> {
        let repo = &self.repo;
        let env = [
            ("GIT_INDEX_FILE", index),
            ("GIT_AUTHOR_NAME", writer.name.as_str()),
            ("GIT_AUTHOR_EMAIL", writer.email.as_str()),
            ("GIT_COMMITTER_NAME", writer.name.as_str()),
            ("GIT_COMMITTER_EMAIL", writer.email.as_str()),
        ];
        let run = |args: &[&str], input: Option<&[u8]>| -> Result<String, TableError> {
            let out = git_with(repo, args, &env, input)?;
            Ok(String::from_utf8_lossy(&out).trim().to_owned())
        };
        run(&["read-tree", "--end-of-options", &state.commit], None)?;
        for i in at {
            let (Some(row), Some((file, mode))) = (state.rows.get(*i), state.files.get(*i)) else {
                return Err(TableError::Source("a located row has no file".into()));
            };
            match change {
                Change::Delete => {
                    run(&["update-index", "--force-remove", "--", file], None)?;
                }
                Change::Update(patch) => {
                    let mut row = row.clone();
                    // a row is read only from a file holding an object
                    row.as_object_mut()
                        .into_iter()
                        .for_each(|o| o.extend(patch.iter().map(|(k, v)| (k.clone(), v.clone()))));
                    let bytes = written(file, &row)?;
                    let blob = run(&["hash-object", "-w", "--stdin"], Some(&bytes))?;
                    run(
                        &[
                            "update-index",
                            "--cacheinfo",
                            &format!("{mode},{blob},{file}"),
                        ],
                        None,
                    )?;
                }
            }
        }
        let tree = run(&["write-tree"], None)?;
        let verb = if *change == Change::Delete {
            "delete"
        } else {
            "update"
        };
        let message = format!("{} {} of {}", verb, at.len(), self.info.set);
        let commit = run(
            &["commit-tree", &tree, "-p", &state.commit, "-m", &message],
            None,
        )?;
        let reference = format!("refs/heads/{}", writer.branch);
        run(&["update-ref", &reference, &commit, &state.commit], None).map_err(|_| {
            TableError::Conflict(format!(
                "the branch {} moved since this set read {}; nothing was changed, reopen the set and retry",
                writer.branch, state.commit
            ))
        })?;
        Ok(commit)
    }
}

/// `row` as the bytes of its file: JSON for `.json`, YAML otherwise (comments are not kept).
fn written(path: &str, row: &Value) -> Result<Vec<u8>, TableError> {
    if path.to_ascii_lowercase().ends_with(".json") {
        // a Value's alternate Display is serde_json's pretty printer, which cannot fail
        Ok(format!("{row:#}\n").into_bytes())
    } else {
        serde_norway::to_string(row)
            .map(String::into_bytes)
            .map_err(|e| TableError::Source(format!("{path} could not be written: {e}")))
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
        &self.info
    }

    fn fetch<'a>(&'a self, plan: &'a Plan, scope: &'a Scope) -> Fetching<'a> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        Box::pin(std::future::ready(Ok::<Fetched, TableError>(answer(
            &state.rows,
            plan,
            scope,
        ))))
    }

    fn etag(&self) -> Option<String> {
        Some(self.commit())
    }

    fn writable(&self) -> bool {
        self.writer.is_some()
    }

    // ponytail: git runs on the calling thread, as `open` does; spawn_blocking when a set's
    // writes grow past a few files.
    fn apply<'a>(&'a self, batch: &'a Batch) -> Applying<'a> {
        let done = match &self.writer {
            Some(writer) => self.commit_batch(writer, batch),
            None => Err(TableError::Forbidden(format!(
                "{} is served read only",
                self.info.set
            ))),
        };
        Box::pin(std::future::ready(done))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;

    use odata_csdl::{
        CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Property, Schema, TypeRef,
    };
    use serde_json::{json, Value};

    use super::{glob, matched, segments, GitLimits, GitTable};
    use crate::{Batch, Change, Cond, Table, TableError, TableInfo};

    fn info() -> TableInfo {
        let mut thing = EntityType::new("Thing");
        thing.key = vec!["id".into()];
        thing.properties = vec![
            Property::new("id", TypeRef::single("Edm.String")),
            Property::new("org", TypeRef::single("Edm.String")),
        ];
        let mut schema = Schema::new("Test");
        schema.entity_types.push(thing);
        let mut container = EntityContainer::new("Service");
        container
            .entity_sets
            .push(EntitySet::new("Things", "Test.Thing"));
        schema.entity_container = Some(container);
        let model = Model {
            version: CsdlVersion::V4_01,
            references: vec![],
            schemas: vec![schema],
        };
        TableInfo::new(Arc::new(model), "Things").expect("a keyed set")
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.org"])
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A repository of `files` (path, contents) committed on `main`, and its commit.
    fn repository(test: &str, files: &[(&str, &str)]) -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("odata-git-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        run(&dir, &["init", "-q", "-b", "main"]);
        for (path, text) in files {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().expect("a parent")).expect("a directory");
            std::fs::write(file, text).expect("a file");
        }
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "--allow-empty", "-m", "rows"]);
        let sha = run(&dir, &["rev-parse", "HEAD"]);
        (dir, sha)
    }

    fn open(repo: &Path, sha: &str, limits: GitLimits) -> Result<GitTable, TableError> {
        GitTable::open(info(), repo, sha, "things/*", limits)
    }

    fn refused(result: Result<GitTable, TableError>) -> TableError {
        result.expect_err("the set must not open")
    }

    #[test]
    fn a_pattern_stays_under_the_root() {
        for bad in [
            "../x/*.yaml",
            "a/../b",
            "/etc/*",
            "a//b",
            "./a",
            "a\\b",
            "a\0b",
            "",
        ] {
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
        assert!(!glob("a*b", "xb"), "the first part is a prefix");
        assert!(!glob("a*x*b", "a-b"), "a middle part must occur");
        assert!(matched(&["spaces", "*", "e.yaml"], "spaces/s1/e.yaml"));
        assert!(!matched(&["spaces", "*", "e.yaml"], "spaces/s1/x/e.yaml"));
        assert!(
            !matched(&["spaces"], "spaces/s1"),
            "a prefix of the path is no match"
        );
    }

    #[test]
    fn a_repository_git_cannot_reach_is_a_source_failure() {
        let sha = "0".repeat(40);
        let bad = refused(open(Path::new("/no\0where"), &sha, GitLimits::strict()));
        assert!(
            matches!(&bad, TableError::Source(why) if why.starts_with("git did not run: ")),
            "{bad:?}"
        );
        let missing = refused(open(
            Path::new("/nonexistent-odata-repo"),
            &sha,
            GitLimits::strict(),
        ));
        assert!(
            matches!(&missing, TableError::Source(why) if why.starts_with("git ls-tree failed: ")),
            "{missing:?}"
        );
    }

    #[test]
    fn every_file_must_be_a_row_within_the_limits() {
        let bad = |why: &str| TableError::BadRequest(why.into());
        let cases: [(&str, &str, TableError); 4] = [
            (
                "things/a.txt",
                "id: a",
                bad("things/a.txt: only .yaml, .yml and .json files are rows"),
            ),
            (
                "things/a.json",
                "[1]",
                bad("things/a.json: a row is an object"),
            ),
            (
                "things/a.yml",
                "- 1",
                bad("things/a.yml: a row is an object"),
            ),
            (
                "things/a.JSON",
                "{",
                bad("things/a.JSON: EOF while parsing an object at line 1 column 1"),
            ),
        ];
        for (i, (path, text, expected)) in cases.into_iter().enumerate() {
            let (repo, sha) = repository(&format!("parse{i}"), &[(path, text)]);
            assert_eq!(refused(open(&repo, &sha, GitLimits::strict())), expected);
        }
        let (repo, sha) = repository("parse-yaml", &[("things/a.yaml", "id: [")]);
        let got = refused(open(&repo, &sha, GitLimits::strict()));
        assert!(
            matches!(&got, TableError::BadRequest(why) if why.starts_with("things/a.yaml: ")),
            "{got:?}"
        );
        let (repo, sha) = repository(
            "limits",
            &[("things/a.yaml", "id: a\n"), ("things/b.yaml", "id: b\n")],
        );
        let limits = GitLimits::strict();
        assert_eq!(
            refused(open(
                &repo,
                &sha,
                GitLimits {
                    max_files: 1,
                    ..limits
                }
            )),
            bad("more than 1 files match")
        );
        assert_eq!(
            refused(open(
                &repo,
                &sha,
                GitLimits {
                    max_file_bytes: 3,
                    ..limits
                }
            )),
            bad("things/a.yaml is over the size this set reads")
        );
        assert_eq!(
            refused(open(
                &repo,
                &sha,
                GitLimits {
                    max_total_bytes: 8,
                    ..limits
                }
            )),
            bad("things/b.yaml is over the size this set reads")
        );
        // a file of exactly the size cap, and files reaching exactly the total cap, are read
        let exact = GitLimits {
            max_file_bytes: 6,
            max_total_bytes: 12,
            ..limits
        };
        assert_eq!(
            open(&repo, &sha, exact).map(|t| t.commit()),
            Ok(sha.clone())
        );
        assert_eq!(
            refused(GitTable::open(
                info(),
                &repo,
                &"g".repeat(40),
                "things/*",
                limits
            )),
            bad("the commit must be a full hexadecimal object name")
        );
        assert_eq!(open(&repo, &sha, limits).map(|t| t.commit()), Ok(sha));
    }

    #[test]
    fn links_are_refused_and_submodules_skipped() {
        let (repo, _) = repository("links", &[("things/a.yaml", "id: a\n")]);
        // a submodule entry is a commit, not a file: the set skips it
        let head = run(&repo, &["rev-parse", "HEAD"]);
        run(
            &repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{head},things/sub"),
            ],
        );
        run(&repo, &["commit", "-q", "-m", "submodule"]);
        let with_sub = run(&repo, &["rev-parse", "HEAD"]);
        let table = open(&repo, &with_sub, GitLimits::strict()).expect("the submodule is skipped");
        assert_eq!(table.commit(), with_sub);
        std::os::unix::fs::symlink("a.yaml", repo.join("things/l.yaml")).expect("a link");
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "link"]);
        let linked = run(&repo, &["rev-parse", "HEAD"]);
        assert_eq!(
            refused(open(&repo, &linked, GitLimits::strict())),
            TableError::BadRequest(
                "things/l.yaml is a symbolic link; a set reads files only".into()
            )
        );
    }

    #[tokio::test]
    async fn a_writer_needs_a_plain_author_and_the_branch_at_the_commit() {
        let (repo, sha) = repository("branch", &[("things/a.json", r#"{"id": "a", "org": "x"}"#)]);
        let set = || open(&repo, &sha, GitLimits::strict()).expect("the set opens");
        let refused_branch = |branch: &str, name: &str, why: &str| {
            assert_eq!(
                set().on_branch(branch, name, "t@example.org").err(),
                Some(TableError::BadRequest(format!("the branch {branch} {why}"))),
                "{branch}"
            );
        };
        refused_branch("main", "a <b>", "or its author is not a plain name");
        refused_branch("-x", "a", "or its author is not a plain name");
        refused_branch("a..b", "a", "is not a valid branch name");
        refused_branch("gone", "a", "does not exist");
        // the read-only set refuses a batch before any git call
        let batch = Batch {
            change: Change::Update(json!({"org": "y"}).as_object().cloned().unwrap_or_default()),
            keys: vec![json!("a")],
            guard: Cond::Const(true),
        };
        assert!(!set().writable());
        assert_eq!(
            set().apply(&batch).await,
            Err(TableError::Forbidden("Things is served read only".into()))
        );
        // a JSON row is written back as JSON
        let writer = set()
            .on_branch("main", "a", "t@example.org")
            .expect("main is at the commit");
        assert!(writer.writable());
        writer.apply(&batch).await.expect("one commit");
        assert_eq!(
            run(&repo, &["log", "-1", "--format=%s", "main"]),
            "update 1 of Things"
        );
        let text = run(&repo, &["show", "main:things/a.json"]);
        assert_eq!(
            serde_json::from_str::<Value>(&text).ok(),
            Some(json!({"id": "a", "org": "y"}))
        );
        refused_branch("main", "a", "has moved past the commit this set reads");
    }

    #[tokio::test]
    async fn a_writer_keeps_its_files_in_step_after_a_delete() {
        let (repo, sha) = repository(
            "delete",
            &[
                ("things/a.yaml", "id: a\norg: x\n"),
                ("things/b.yaml", "id: b\norg: x\n"),
            ],
        );
        let writer = open(&repo, &sha, GitLimits::strict())
            .expect("the set opens")
            .on_branch("main", "a", "t@example.org")
            .expect("main is at the commit");
        let batch = |change, key: &str| Batch {
            change,
            keys: vec![json!(key)],
            guard: Cond::Const(true),
        };
        writer
            .apply(&batch(Change::Delete, "a"))
            .await
            .expect("a deleted");
        assert_eq!(
            run(&repo, &["log", "-1", "--format=%s", "main"]),
            "delete 1 of Things"
        );
        let org_y = json!({"org": "y"}).as_object().cloned().unwrap_or_default();
        writer
            .apply(&batch(Change::Update(org_y), "b"))
            .await
            .expect("b updated");
        assert_eq!(
            run(&repo, &["ls-tree", "--name-only", "-r", "main"]),
            "things/b.yaml"
        );
        let text = run(&repo, &["show", "main:things/b.yaml"]);
        assert_eq!(
            serde_norway::from_str::<Value>(&text).ok(),
            Some(json!({"id": "b", "org": "y"}))
        );
    }
}
