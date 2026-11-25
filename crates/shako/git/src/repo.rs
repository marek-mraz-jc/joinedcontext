#![forbid(unsafe_code)]
//! Isolated bare Git repository lifecycle, per-repository write locks, tree/commit/file reads,
//! and pack-backed API commits (S2, S11, R§2.4, R§2.5, Threat T1, T2).

use crate::{git_err, storage_err};
use bstr::{BString, ByteSlice};
use gix_hash::ObjectId;
use gix_object::WriteTo;
use gix_object::tree::EntryKind;
use gix_pack::data::output;
use gix_ref::{
    FullName, Target,
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use shako_core::{
    InstanceId, RepoId, ShakoError, ShakoResult, repo_disk_path, validate_branch_name,
    validate_full_ref_name, validate_tree_path,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Safe minimal `config` written by `shako` to every bare repository at init (S2, R§2.5, Threat T2).
/// Disables auto-gc, hooks, fsmonitor, and symlinks.
pub const SAFE_BARE_REPO_CONFIG: &str = "\
[core]
\trepositoryformatversion = 0
\tfilemode = true
\tbare = true
\tlogallrefupdates = false
\tsymlinks = false
\thooksPath = /dev/null
\tfsmonitor = false
[receive]
\tautogc = false
[gc]
\tauto = 0
";

/// Per-repository async write lock registry so concurrent receive-pack, API commits, merges,
/// and compactions on the same `RepoId` serialize cleanly without racing (S4, S6, Threat T15).
#[derive(Clone, Default)]
pub struct RepoLocks {
    locks: Arc<Mutex<BTreeMap<RepoId, Arc<AsyncMutex<()>>>>>,
}

impl RepoLocks {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn lock(&self, repo_id: RepoId) -> OwnedMutexGuard<()> {
        let mutex = {
            let mut map = self.locks.lock();
            map.entry(repo_id)
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        mutex.lock_owned().await
    }

    pub fn try_lock(&self, repo_id: RepoId) -> Option<OwnedMutexGuard<()>> {
        let mutex = {
            let mut map = self.locks.lock();
            map.entry(repo_id)
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        mutex.try_lock_owned().ok()
    }
}

/// Opens a bare repository at `path` in strict isolated mode (`gix::open::Options::isolated()`),
/// ignoring environment variables, global/system gitconfig, hooks, and fsmonitor (Threat T2).
pub fn open_isolated(path: &Path) -> ShakoResult<gix::Repository> {
    let mut opts = gix::open::Options::isolated();
    // 8 MiB of delta bases per handle, in the cache that counts what it allocates (S22, T-7522).
    // gix's hashmap caches (`core.deltaBaseCacheLimit`, the object cache) count an entry's
    // length and reuse evicted buffers whole, so each entry could hold the largest blob seen.
    opts = opts
        .open_path_as_is(true)
        .config_overrides(["gitoxide.core.deltaBaseCacheLimit=8m"]);
    let repo = gix::open_opts(path, opts).map_err(git_err("failed to open repository"))?;
    // Without a free list a small object never holds a blob's buffer, so a job holds the two
    // largest objects `pack::job_bytes` admitted (+16 MiB on a delta chain, +24 MiB with one).
    Ok(repo.without_freelist())
}

/// `refs/heads/{branch}` once [`validate_branch_name`] admits `branch`.
pub(crate) fn branch_ref(branch: &str) -> ShakoResult<FullName> {
    validate_branch_name(branch)?;
    full_ref(&format!("refs/heads/{branch}"))
}

/// `refname` once [`validate_full_ref_name`] admits it; gix checks a subset of the same rules.
pub(crate) fn full_ref(refname: &str) -> ShakoResult<FullName> {
    validate_full_ref_name(refname)?;
    FullName::try_from(refname).map_err(git_err("ref name"))
}

/// Initializes a bare repository at `{root}/{instance_id}/{repo_id}.git`, writes `SAFE_BARE_REPO_CONFIG`,
/// sets `HEAD` to `refs/heads/{default_branch}`, and optionally writes an initial commit (Row 2).
pub fn init_bare_repo(
    root: &Path,
    instance_id: InstanceId,
    repo_id: RepoId,
    default_branch: &str,
    auto_init: bool,
    description: &str,
) -> ShakoResult<PathBuf> {
    let head_target = branch_ref(default_branch)?;
    let path = repo_disk_path(root, instance_id, repo_id)?;
    std::fs::create_dir_all(path.parent().unwrap_or(root))
        .map_err(storage_err("failed to create instance dir"))?;
    let repo = gix::init_bare(&path).map_err(git_err("gix::init_bare failed"))?;
    std::fs::write(path.join("config"), SAFE_BARE_REPO_CONFIG)
        .map_err(storage_err("failed to write repo config"))?;
    std::fs::write(path.join("description"), description)
        .map_err(storage_err("failed to write repo description"))?;

    let head_name = FullName::try_from("HEAD").map_err(git_err("HEAD"))?;
    repo.refs
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "init".into(),
                    },
                    expected: PreviousValue::Any,
                    new: Target::Symbolic(head_target),
                },
                name: head_name,
                deref: false,
            }],
            gix::lock::acquire::Fail::Immediately,
            gix::lock::acquire::Fail::Immediately,
        )
        .and_then(|tx| tx.commit(None))
        .map_err(git_err("update HEAD"))?;

    if auto_init {
        let readme = format!(
            "# Repository\n\n{}\n",
            if description.is_empty() {
                "Initialized by shako."
            } else {
                description
            }
        );
        commit_file_changes(
            &path,
            default_branch,
            "Initial commit",
            &CommitAuthor {
                name: "shako".into(),
                email: "shako@joinedcontext.local".into(),
            },
            &CommitAuthor {
                name: "shako".into(),
                email: "shako@joinedcontext.local".into(),
            },
            &[FileChangeOp {
                operation: FileOpKind::Create,
                path: "README.md".into(),
                content: Some(readme.into_bytes()),
                sha: None,
            }],
        )?;
    }

    Ok(path)
}

/// Copies all objects and references from `origin_path` into a newly initialized bare repo at `dest_path`
/// (Row 3: `POST /api/v1/repos/migrate` intra-forge copy, S7).
pub fn copy_bare_repo(
    origin_path: &Path,
    dest_path: &Path,
    default_branch: &str,
) -> ShakoResult<()> {
    let origin = open_isolated(origin_path)?;
    let dest = open_isolated(dest_path)?;

    // Copy pack files and loose objects
    let origin_pack_dir = origin_path.join("objects/pack");
    let dest_pack_dir = dest_path.join("objects/pack");
    std::fs::create_dir_all(&dest_pack_dir).map_err(storage_err("mkdir pack dir"))?;
    if origin_pack_dir.is_dir() {
        for entry in std::fs::read_dir(&origin_pack_dir)
            .map_err(storage_err("read pack dir"))?
            .flatten()
        {
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            // A pack compaction superseded has no index and is left; a cruft pack's `.seen` times
            // go with it, so the copy does not give its objects a new prune grace.
            let served =
                !name_str.ends_with(".pack") || entry.path().with_extension("idx").is_file();
            let pack_file = [".pack", ".idx", ".seen"]
                .iter()
                .any(|ext| name_str.ends_with(ext));
            if pack_file && served {
                std::fs::copy(entry.path(), dest_pack_dir.join(&file_name))
                    .map_err(storage_err("copy pack"))?;
            }
        }
    }

    let mut edits = Vec::new();
    let platform = origin.references().map_err(git_err("references"))?;
    let all_refs = platform.all().map_err(git_err("all refs"))?;
    for r in all_refs.flatten() {
        let name_str = r.name().as_bstr().to_str_lossy().into_owned();
        if !(name_str.starts_with("refs/heads/") || name_str.starts_with("refs/tags/")) {
            continue;
        }
        if let Some(id) = r.try_id() {
            let full_name: FullName = name_str.try_into().map_err(git_err("ref name"))?;
            edits.push(RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "migrate".into(),
                    },
                    expected: PreviousValue::Any,
                    new: Target::Object(id.detach()),
                },
                name: full_name,
                deref: false,
            });
        }
    }

    let head_target = branch_ref(default_branch)?;
    let head_name = FullName::try_from("HEAD").map_err(git_err("HEAD"))?;
    edits.push(RefEdit {
        change: Change::Update {
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: false,
                message: "migrate".into(),
            },
            expected: PreviousValue::Any,
            new: Target::Symbolic(head_target),
        },
        name: head_name,
        deref: false,
    });

    dest.refs
        .transaction()
        .prepare(
            edits,
            gix::lock::acquire::Fail::Immediately,
            gix::lock::acquire::Fail::Immediately,
        )
        .and_then(|tx| tx.commit(None))
        .map_err(git_err("migrate refs"))?;

    Ok(())
}

/// Computes on-disk size in bytes of `{repo_path}/objects`.
#[must_use]
pub fn compute_repo_size_bytes(repo_path: &Path) -> u64 {
    fn dir_bytes(dir: &Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut sum = 0u64;
        let entries = rd
            .flatten()
            .filter_map(|e| Some((e.path(), e.metadata().ok()?)));
        for (path, meta) in entries {
            if meta.is_dir() {
                sum = sum.saturating_add(dir_bytes(&path));
            } else {
                sum = sum.saturating_add(meta.len());
            }
        }
        sum
    }
    dir_bytes(&repo_path.join("objects"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitAuthor {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileOpKind {
    Create,
    Update,
    Delete,
    Upload,
}

#[derive(Debug, Clone)]
pub struct FileChangeOp {
    pub operation: FileOpKind,
    pub path: String,
    pub content: Option<Vec<u8>>,
    pub sha: Option<String>,
}

/// Writes a set of in-memory objects (`(ObjectId, gix_object::Kind, Vec<u8>)`) as a single
/// packfile + index into `{repo_path}/objects/pack/` using `gix_pack::Bundle::write_to_directory`,
/// so API commits and merges never leave loose objects on disk (R§2.5, S11).
pub fn write_objects_as_pack(
    repo_path: &Path,
    objects: &[(ObjectId, gix_object::Kind, Vec<u8>)],
) -> ShakoResult<()> {
    if objects.is_empty() {
        return Ok(());
    }
    let mut entries = Vec::with_capacity(objects.len());
    for (oid, kind, raw) in objects {
        let count = output::Count::from_data(*oid, None);
        let data = gix_object::Data::new(raw.as_slice(), *kind, gix_hash::Kind::Sha1);
        let entry = output::Entry::from_data(&count, &data, gix::zlib::Compression::default())
            .map_err(git_err("pack entry deflate failed"))?;
        entries.push(entry);
    }

    let mut pack_bytes = Vec::new();
    let num_entries = u32::try_from(entries.len()).map_err(crate::pack::too_many_objects)?;
    let iter = output::bytes::FromEntriesIter::new(
        std::iter::once(gix::Result::Ok(entries)),
        &mut pack_bytes,
        num_entries,
        gix_pack::data::Version::V2,
        gix_hash::Kind::Sha1,
    );
    for res in iter {
        res.map_err(git_err("pack encode failed"))?;
    }

    let pack_dir = repo_path.join("objects/pack");
    std::fs::create_dir_all(&pack_dir).map_err(storage_err("mkdir pack dir"))?;
    let mut cursor = std::io::Cursor::new(pack_bytes);
    let mut progress = gix::progress::Discard;
    let should_interrupt = AtomicBool::new(false);
    let outcome = gix_pack::Bundle::write_to_directory(
        &mut cursor,
        Some(&pack_dir),
        &mut progress,
        &should_interrupt,
        None::<gix_odb::Handle>,
        gix_hash::Kind::Sha1,
        gix_pack::bundle::write::Options {
            thread_limit: Some(1),
            iteration_mode: gix_pack::data::input::Mode::Verify,
            index_version: gix_pack::index::Version::V2,
            alloc_limit_bytes: Some(32 * 1024 * 1024),
            compression: gix::zlib::Compression::default(),
        },
    )
    .map_err(git_err("write pack bundle failed"))?;
    let _ = outcome.keep_path.map(std::fs::remove_file);
    Ok(())
}

fn hash_and_serialize_object(
    kind: gix_object::Kind,
    obj: &impl WriteTo,
) -> ShakoResult<(ObjectId, Vec<u8>)> {
    let mut raw = Vec::new();
    obj.write_to(&mut raw)
        .map_err(git_err("serialize object"))?;
    let oid = gix_object::compute_hash(gix_hash::Kind::Sha1, kind, &raw)
        .map_err(git_err("compute hash"))?;
    Ok((oid, raw))
}

fn hash_blob_bytes(bytes: &[u8]) -> ShakoResult<ObjectId> {
    gix_object::compute_hash(gix_hash::Kind::Sha1, gix_object::Kind::Blob, bytes)
        .map_err(git_err("compute blob hash"))
}

/// Resolves a ref/branch/tag/SHA string to its peeled commit `ObjectId`.
pub fn resolve_commit_oid(repo: &gix::Repository, git_ref: &str) -> ShakoResult<ObjectId> {
    let trimmed = git_ref.trim();
    if trimmed.len() == 40
        && trimmed.bytes().all(|b| b.is_ascii_hexdigit())
        && let Ok(oid) = ObjectId::from_hex(trimmed.as_bytes())
        && repo.find_commit(oid).is_ok()
    {
        return Ok(oid);
    }
    for candidate in [
        trimmed.to_owned(),
        format!("refs/heads/{trimmed}"),
        format!("refs/tags/{trimmed}"),
        format!("refs/pull/{trimmed}"),
    ] {
        let Ok(partial) = <&gix_ref::PartialNameRef>::try_from(candidate.as_str()) else {
            continue;
        };
        if let Ok(Some(mut r)) = repo.try_find_reference(partial)
            && let Ok(commit) = r.peel_to_commit()
        {
            return Ok(commit.id);
        }
    }
    if let Ok(id) = repo.rev_parse_single(trimmed)
        && let Ok(obj) = id.object()
        && let Ok(commit) = obj.peel_to_kind(gix_object::Kind::Commit)
    {
        return Ok(commit.id);
    }
    Err(ShakoError::NotFound(format!("ref '{git_ref}' not found")))
}

/// Applies one or more file operations (`Create`, `Update`, `Delete`, `Upload`) on `branch`
/// with all-or-nothing blob SHA preconditions, writes one small packfile, and updates `refs/heads/{branch}` (S11).
pub fn commit_file_changes(
    repo_path: &Path,
    branch: &str,
    message: &str,
    author: &CommitAuthor,
    committer: &CommitAuthor,
    changes: &[FileChangeOp],
) -> ShakoResult<String> {
    let full_ref = branch_ref(branch)?;
    if changes.is_empty() {
        return Err(ShakoError::Unprocessable(
            "no file changes specified".into(),
        ));
    }
    let repo = open_isolated(repo_path)?;

    let parent_commit_oid = match repo.refs.try_find(full_ref.as_bstr()) {
        Ok(Some(r)) => r.target.try_id().map(ToOwned::to_owned),
        _ => None,
    };

    // Load flat path -> (EntryKind, ObjectId) map from parent commit's tree
    let mut tree_entries: BTreeMap<String, (EntryKind, ObjectId)> = BTreeMap::new();
    if let Some(parent_oid) = parent_commit_oid {
        let commit = repo
            .find_commit(parent_oid)
            .map_err(git_err("find parent commit"))?;
        let tree = commit.tree().map_err(git_err("find parent tree"))?;
        collect_tree_blobs(&repo, &tree, "", &mut tree_entries)?;
    }

    let mut new_objects: Vec<(ObjectId, gix_object::Kind, Vec<u8>)> = Vec::new();

    // All-or-nothing precondition verification and blob creation
    for op in changes {
        let clean_path = validate_tree_path(&op.path)?;
        let existing = tree_entries.get(&clean_path).map(|(_, oid)| *oid);
        match op.operation {
            FileOpKind::Create => {
                if existing.is_some() {
                    return Err(ShakoError::Unprocessable(format!(
                        "file '{clean_path}' already exists"
                    )));
                }
                let content = op.content.as_deref().unwrap_or_default();
                let oid = hash_blob_bytes(content)?;
                new_objects.push((oid, gix_object::Kind::Blob, content.to_vec()));
                tree_entries.insert(clean_path, (EntryKind::Blob, oid));
            }
            FileOpKind::Update => {
                let Some(cur_oid) = existing else {
                    return Err(ShakoError::NotFound(format!(
                        "file '{clean_path}' does not exist"
                    )));
                };
                let Some(expected_sha) = op.sha.as_deref().filter(|s| !s.is_empty()) else {
                    return Err(ShakoError::Unprocessable(format!(
                        "update of '{clean_path}' requires blob sha"
                    )));
                };
                if cur_oid.to_hex() != expected_sha {
                    return Err(ShakoError::Conflict(format!(
                        "file '{clean_path}' sha mismatch: expected {expected_sha}, found {cur_oid}"
                    )));
                }
                let content = op.content.as_deref().unwrap_or_default();
                let oid = hash_blob_bytes(content)?;
                new_objects.push((oid, gix_object::Kind::Blob, content.to_vec()));
                tree_entries.insert(clean_path, (EntryKind::Blob, oid));
            }
            FileOpKind::Upload => {
                if let Some(expected_sha) = op.sha.as_deref().filter(|s| !s.is_empty()) {
                    let Some(cur_oid) = existing else {
                        return Err(ShakoError::Conflict(format!(
                            "file '{clean_path}' does not exist (expected sha {expected_sha})"
                        )));
                    };
                    if cur_oid.to_hex() != expected_sha {
                        return Err(ShakoError::Conflict(format!(
                            "file '{clean_path}' sha mismatch: expected {expected_sha}, found {cur_oid}"
                        )));
                    }
                }
                let content = op.content.as_deref().unwrap_or_default();
                let oid = hash_blob_bytes(content)?;
                new_objects.push((oid, gix_object::Kind::Blob, content.to_vec()));
                tree_entries.insert(clean_path, (EntryKind::Blob, oid));
            }
            FileOpKind::Delete => {
                let Some(cur_oid) = existing else {
                    return Err(ShakoError::NotFound(format!(
                        "file '{clean_path}' does not exist"
                    )));
                };
                let Some(expected_sha) = op.sha.as_deref().filter(|s| !s.is_empty()) else {
                    return Err(ShakoError::Unprocessable(format!(
                        "delete of '{clean_path}' requires blob sha"
                    )));
                };
                if cur_oid.to_hex() != expected_sha {
                    return Err(ShakoError::Conflict(format!(
                        "file '{clean_path}' sha mismatch: expected {expected_sha}, found {cur_oid}"
                    )));
                }
                tree_entries.remove(&clean_path);
            }
        }
    }

    let root_tree_oid = build_nested_trees(&tree_entries, &mut new_objects)?;
    let now_secs = jc_time::TimestampTz::now().as_second();
    let time = gix::date::Time::new(now_secs, 0);
    let author_sig = gix::actor::Signature {
        name: BString::from(sanitize_actor_field(&author.name, "shako")),
        email: BString::from(sanitize_actor_field(&author.email, "shako@local")),
        time,
    };
    let committer_sig = gix::actor::Signature {
        name: BString::from(sanitize_actor_field(&committer.name, "shako")),
        email: BString::from(sanitize_actor_field(&committer.email, "shako@local")),
        time,
    };
    let parents = parent_commit_oid.into_iter().collect();
    let commit_obj = gix_object::Commit {
        tree: root_tree_oid,
        parents,
        author: author_sig,
        committer: committer_sig,
        encoding: None,
        message: BString::from(message),
        extra_headers: Vec::new(),
    };
    let (commit_oid, commit_raw) =
        hash_and_serialize_object(gix_object::Kind::Commit, &commit_obj)?;
    new_objects.push((commit_oid, gix_object::Kind::Commit, commit_raw));

    // Write all new blobs, trees, and the commit as one packfile
    write_objects_as_pack(repo_path, &new_objects)?;

    let expected = match parent_commit_oid {
        Some(old) => PreviousValue::MustExistAndMatch(Target::Object(old)),
        None => PreviousValue::MustNotExist,
    };
    repo.refs
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "commit".into(),
                    },
                    expected,
                    new: Target::Object(commit_oid),
                },
                name: full_ref,
                deref: false,
            }],
            gix::lock::acquire::Fail::Immediately,
            gix::lock::acquire::Fail::Immediately,
        )
        .and_then(|tx| tx.commit(None))
        .map_err(|e| ShakoError::Conflict(format!("concurrent ref update on {branch}: {e}")))?;

    Ok(commit_oid.to_hex().to_string())
}

pub(crate) fn sanitize_actor_field(s: &str, fallback: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | '\n' | '\r' | '\0'))
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Deepest tree nesting a walk follows (Git's own `core.maxTreeDepth` defaults to 2048 and real
/// repositories stay far below 100).
pub(crate) use shako_core::MAX_TREE_DEPTH;
/// Most tree entries one walk visits: a tree naming one subtree many times grows the path count
/// exponentially from a pack of a few KB.
pub(crate) const MAX_TREE_ENTRIES: usize = 500_000;
/// Longest path a walk builds (Linux `PATH_MAX`): a chain of long directory names otherwise makes
/// every path below it tens of KB.
pub(crate) const MAX_TREE_PATH_BYTES: usize = 4096;
/// Most path bytes one walk holds, so entries times path length stays bounded too.
pub(crate) const MAX_TREE_WALK_BYTES: usize = 64 << 20;

/// What one tree walk has visited so far, checked against the limits above.
#[derive(Default)]
struct TreeWalk {
    entries: usize,
    path_bytes: usize,
}

impl TreeWalk {
    fn enter(&self, depth: usize) -> ShakoResult<()> {
        if depth > MAX_TREE_DEPTH {
            return Err(ShakoError::Unprocessable(format!(
                "tree nesting deeper than {MAX_TREE_DEPTH} levels"
            )));
        }
        Ok(())
    }

    fn visit(&mut self, path: &str) -> ShakoResult<()> {
        self.entries += 1;
        self.path_bytes += path.len();
        if self.entries > MAX_TREE_ENTRIES {
            return Err(ShakoError::Unprocessable(format!(
                "tree holds more than {MAX_TREE_ENTRIES} entries"
            )));
        }
        if path.len() > MAX_TREE_PATH_BYTES || self.path_bytes > MAX_TREE_WALK_BYTES {
            return Err(ShakoError::Unprocessable(format!(
                "tree paths longer than {MAX_TREE_PATH_BYTES} bytes or {MAX_TREE_WALK_BYTES} bytes in all"
            )));
        }
        Ok(())
    }
}

/// Collects every blob path under `tree`, refusing a tree past [`MAX_TREE_DEPTH`] or
/// [`MAX_TREE_ENTRIES`] with [`ShakoError::Unprocessable`].
pub(crate) fn collect_tree_blobs(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &str,
    out: &mut BTreeMap<String, (EntryKind, ObjectId)>,
) -> ShakoResult<()> {
    walk_tree(repo, tree, prefix, 0, &mut TreeWalk::default(), out)
}

fn walk_tree(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &str,
    depth: usize,
    walk: &mut TreeWalk,
    out: &mut BTreeMap<String, (EntryKind, ObjectId)>,
) -> ShakoResult<()> {
    walk.enter(depth)?;
    let decoded = tree.decode().map_err(git_err("decode tree"))?;
    for entry in decoded.entries {
        let name = entry.filename.to_str_lossy();
        let full = if prefix.is_empty() {
            name.into_owned()
        } else {
            format!("{prefix}/{name}")
        };
        walk.visit(&full)?;
        if entry.mode.is_tree() {
            let sub = repo.find_tree(entry.oid).map_err(git_err("find subtree"))?;
            walk_tree(repo, &sub, &full, depth + 1, walk, out)?;
        } else {
            // A gitlink (submodule) names a commit of another repository: kept as such so a
            // rebuilt tree holds it, never looked up as a blob here.
            let kind = if entry.mode.is_commit() {
                EntryKind::Commit
            } else if entry.mode.is_executable() {
                EntryKind::BlobExecutable
            } else if entry.mode.is_link() {
                EntryKind::Link
            } else {
                EntryKind::Blob
            };
            out.insert(full, (kind, entry.oid.to_owned()));
        }
    }
    Ok(())
}

/// Collects every non-tree entry of `tree` that no tree of `parents` holds at the same path with
/// the same id, skipping a subtree a parent holds unchanged; the limits of [`collect_tree_blobs`]
/// apply to the entries it visits.
pub(crate) fn collect_changed_blobs(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    parents: &[gix::Tree<'_>],
    out: &mut BTreeMap<String, ObjectId>,
) -> ShakoResult<()> {
    walk_changed(repo, tree, parents, "", 0, &mut TreeWalk::default(), out)
}

fn walk_changed(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    parents: &[gix::Tree<'_>],
    prefix: &str,
    depth: usize,
    walk: &mut TreeWalk,
    out: &mut BTreeMap<String, ObjectId>,
) -> ShakoResult<()> {
    walk.enter(depth)?;
    let held = parents
        .iter()
        .map(|t| {
            let d = t.decode().map_err(git_err("decode tree"))?;
            Ok(d.entries
                .into_iter()
                .map(|e| (e.filename, e))
                .collect::<BTreeMap<_, _>>())
        })
        .collect::<ShakoResult<Vec<_>>>()?;
    let decoded = tree.decode().map_err(git_err("decode tree"))?;
    for entry in decoded.entries {
        let name = entry.filename.to_str_lossy();
        let full = if prefix.is_empty() {
            name.into_owned()
        } else {
            format!("{prefix}/{name}")
        };
        walk.visit(&full)?;
        let same: Vec<_> = held.iter().filter_map(|h| h.get(entry.filename)).collect();
        if same
            .iter()
            .any(|e| e.oid == entry.oid && e.mode.is_tree() == entry.mode.is_tree())
        {
            continue;
        }
        if entry.mode.is_tree() {
            let find = |oid| repo.find_tree(oid).map_err(git_err("find subtree"));
            let sub = find(entry.oid)?;
            let subs = same
                .iter()
                .filter(|e| e.mode.is_tree())
                .map(|e| find(e.oid))
                .collect::<ShakoResult<Vec<_>>>()?;
            walk_changed(repo, &sub, &subs, &full, depth + 1, walk, out)?;
        } else if !entry.mode.is_commit() {
            out.insert(full, entry.oid.to_owned());
        }
    }
    Ok(())
}

/// Builds a hierarchy of `gix_object::Tree` objects bottom-up from a flat map of `path -> (EntryKind, ObjectId)`.
pub(crate) fn build_nested_trees(
    files: &BTreeMap<String, (EntryKind, ObjectId)>,
    new_objects: &mut Vec<(ObjectId, gix_object::Kind, Vec<u8>)>,
) -> ShakoResult<ObjectId> {
    #[derive(Default)]
    struct DirNode {
        files: BTreeMap<String, (EntryKind, ObjectId)>,
        subdirs: BTreeMap<String, DirNode>,
    }

    fn insert_path(node: &mut DirNode, path: &str, kind: EntryKind, oid: ObjectId) {
        match path.split_once('/') {
            None => {
                node.files.insert(path.to_owned(), (kind, oid));
            }
            Some((dir, rest)) => {
                let child = node.subdirs.entry(dir.to_owned()).or_default();
                insert_path(child, rest, kind, oid);
            }
        }
    }

    fn write_dir(
        node: &DirNode,
        new_objects: &mut Vec<(ObjectId, gix_object::Kind, Vec<u8>)>,
    ) -> ShakoResult<ObjectId> {
        let mut entries = Vec::with_capacity(node.files.len() + node.subdirs.len());
        for (name, (kind, oid)) in &node.files {
            entries.push(gix_object::tree::Entry {
                mode: (*kind).into(),
                filename: BString::from(name.as_str()),
                oid: *oid,
            });
        }
        for (dir_name, child) in &node.subdirs {
            let child_oid = write_dir(child, new_objects)?;
            entries.push(gix_object::tree::Entry {
                mode: EntryKind::Tree.into(),
                filename: BString::from(dir_name.as_str()),
                oid: child_oid,
            });
        }
        // Git sorts tree entries in tree-sort order (directories compared with trailing '/')
        entries.sort_by(|a, b| {
            let a_is_tree = a.mode.is_tree();
            let b_is_tree = b.mode.is_tree();
            tree_entry_cmp(
                a.filename.as_slice(),
                a_is_tree,
                b.filename.as_slice(),
                b_is_tree,
            )
        });
        let tree_obj = gix_object::Tree { entries };
        let (oid, raw) = hash_and_serialize_object(gix_object::Kind::Tree, &tree_obj)?;
        new_objects.push((oid, gix_object::Kind::Tree, raw));
        Ok(oid)
    }

    let mut root = DirNode::default();
    for (path, (kind, oid)) in files {
        insert_path(&mut root, path, *kind, *oid);
    }
    write_dir(&root, new_objects)
}

fn tree_entry_cmp(a: &[u8], a_tree: bool, b: &[u8], b_tree: bool) -> std::cmp::Ordering {
    let min_len = a.len().min(b.len());
    let cmp = a[..min_len].cmp(&b[..min_len]);
    if cmp != std::cmp::Ordering::Equal {
        return cmp;
    }
    let a_next = a.get(min_len).copied().or_else(|| a_tree.then_some(b'/'));
    let b_next = b.get(min_len).copied().or_else(|| b_tree.then_some(b'/'));
    a_next.cmp(&b_next)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntryInfo {
    pub path: String,
    pub mode: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub size: u64,
    pub sha: String,
}

/// Lists entries of a commit's tree (recursive or top-level) for Row 21 (`GET git/trees/{ref}`).
pub fn list_tree_entries(
    repo_path: &Path,
    git_ref: &str,
    recursive: bool,
) -> ShakoResult<(String, Vec<TreeEntryInfo>)> {
    let repo = open_isolated(repo_path)?;
    // Support resolving either a commit/branch/tag ref or a direct tree SHA
    let tree = if git_ref.len() == 40
        && git_ref.bytes().all(|b| b.is_ascii_hexdigit())
        && let Ok(oid) = ObjectId::from_hex(git_ref.as_bytes())
        && let Ok(t) = repo.find_tree(oid)
    {
        t
    } else {
        let commit_oid = resolve_commit_oid(&repo, git_ref)?;
        let commit = repo
            .find_commit(commit_oid)
            .map_err(git_err("find commit"))?;
        commit.tree().map_err(git_err("find tree"))?
    };

    let tree_sha = tree.id.to_hex().to_string();
    let mut out = Vec::new();
    let walk = &mut TreeWalk::default();
    walk_tree_entries(&repo, &tree, "", recursive, 0, walk, &mut out)?;
    Ok((tree_sha, out))
}

fn walk_tree_entries(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &str,
    recursive: bool,
    depth: usize,
    walk: &mut TreeWalk,
    out: &mut Vec<TreeEntryInfo>,
) -> ShakoResult<()> {
    walk.enter(depth)?;
    let decoded = tree.decode().map_err(git_err("decode tree"))?;
    for entry in decoded.entries {
        let name = entry.filename.to_str_lossy();
        let path = if prefix.is_empty() {
            name.into_owned()
        } else {
            format!("{prefix}/{name}")
        };
        walk.visit(&path)?;
        let is_tree = entry.mode.is_tree();
        let (mode_str, type_str, size) = if is_tree {
            ("040000".to_owned(), "tree".to_owned(), 0)
        } else {
            let sz = repo.find_header(entry.oid).map(|h| h.size()).unwrap_or(0);
            let mode = if entry.mode.is_executable() {
                "100755"
            } else if entry.mode.is_link() {
                "120000"
            } else {
                "100644"
            };
            (mode.to_owned(), "blob".to_owned(), sz)
        };
        out.push(TreeEntryInfo {
            path: path.clone(),
            mode: mode_str,
            entry_type: type_str,
            size,
            sha: entry.oid.to_hex().to_string(),
        });
        if is_tree && recursive {
            let sub = repo.find_tree(entry.oid).map_err(git_err("find subtree"))?;
            walk_tree_entries(repo, &sub, &path, true, depth + 1, walk, out)?;
        }
    }
    Ok(())
}

/// Reads a single file's bytes and blob SHA at `git_ref` (Row 25: `GET contents/{path}?ref=`).
pub fn read_file_at_ref(
    repo_path: &Path,
    git_ref: &str,
    file_path: &str,
    admit: crate::pack::Admit<'_>,
) -> ShakoResult<Option<(String, Vec<u8>)>> {
    let clean = validate_tree_path(file_path)?;
    let repo = open_isolated(repo_path)?;
    let commit_oid = resolve_commit_oid(&repo, git_ref)?;
    let commit = repo
        .find_commit(commit_oid)
        .map_err(git_err("find commit"))?;
    let mut tree = commit.tree().map_err(git_err("find tree"))?;
    let parts: Vec<&str> = clean.split('/').collect();
    let Some(entry) = tree.peel_to_entry(parts).map_err(git_err("lookup entry"))? else {
        return Ok(None);
    };
    if entry.mode().is_tree() {
        return Ok(None);
    }
    let header = repo
        .find_header(entry.oid())
        .map_err(git_err("find blob"))?;
    // The blob and the encoded form a caller makes of it.
    admit(header.size().saturating_mul(3))?;
    let mut blob = repo.find_blob(entry.oid()).map_err(git_err("find blob"))?;
    Ok(Some((
        blob.id.to_hex().to_string(),
        std::mem::take(&mut blob.data),
    )))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_date: String,
    pub parent_shas: Vec<String>,
}

/// Reads a single commit by SHA or ref (Row 35: `GET git/commits/{sha}`).
pub fn read_commit_info(repo_path: &Path, sha_or_ref: &str) -> ShakoResult<CommitInfo> {
    let repo = open_isolated(repo_path)?;
    let oid = resolve_commit_oid(&repo, sha_or_ref)?;
    let commit = repo.find_commit(oid).map_err(git_err("find commit"))?;
    decode_commit_info(&commit)
}

fn decode_commit_info(commit: &gix::Commit<'_>) -> ShakoResult<CommitInfo> {
    let decoded = commit.decode().map_err(git_err("decode commit"))?;
    let author = decoded.author().unwrap_or_default();
    let committer = decoded.committer().unwrap_or_default();
    let author_dt =
        jc_time::TimestampTz::from_second(author.time().map(|t| t.seconds).unwrap_or(0))
            .unwrap_or_else(jc_time::TimestampTz::now)
            .to_rfc3339();
    let committer_dt =
        jc_time::TimestampTz::from_second(committer.time().map(|t| t.seconds).unwrap_or(0))
            .unwrap_or_else(jc_time::TimestampTz::now)
            .to_rfc3339();
    Ok(CommitInfo {
        sha: commit.id.to_hex().to_string(),
        message: decoded.message.to_str_lossy().into_owned(),
        author_name: author.name.to_str_lossy().into_owned(),
        author_email: author.email.to_str_lossy().into_owned(),
        author_date: author_dt,
        committer_name: committer.name.to_str_lossy().into_owned(),
        committer_email: committer.email.to_str_lossy().into_owned(),
        committer_date: committer_dt,
        parent_shas: decoded.parents().map(|p| p.to_hex().to_string()).collect(),
    })
}

/// Walks commit history from `git_ref`, optionally filtered to commits touching `path_filter`,
/// capped at `max_walk` (10,000 commits, S9 & S11 Row 28).
pub fn list_commits(
    repo_path: &Path,
    git_ref: &str,
    path_filter: Option<&str>,
    limit: usize,
    max_walk: usize,
) -> ShakoResult<Vec<CommitInfo>> {
    let repo = open_isolated(repo_path)?;
    let start_oid = resolve_commit_oid(&repo, git_ref)?;
    let filter_path = match path_filter
        .map(|p| p.trim_matches('/'))
        .filter(|p| !p.is_empty())
    {
        Some(p) => Some(validate_tree_path(p)?),
        None => None,
    };

    let walk = repo
        .rev_walk([start_oid])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()
        .map_err(git_err("rev_walk failed"))?;

    let mut out = Vec::new();
    for (walked, item) in walk.enumerate() {
        if walked >= max_walk || out.len() >= limit {
            break;
        }
        // A commit whose parent is gone ends its line of history; the walk goes on elsewhere.
        let Some(commit) = item.ok().and_then(|info| info.object().ok()) else {
            continue;
        };
        if let Some(ref fpath) = filter_path
            && !commit_touches_path(&repo, &commit, fpath)
        {
            continue;
        }
        if let Ok(ci) = decode_commit_info(&commit) {
            out.push(ci);
        }
    }
    Ok(out)
}

fn commit_touches_path(repo: &gix::Repository, commit: &gix::Commit<'_>, path: &str) -> bool {
    let Ok(mut tree) = commit.tree() else {
        return false;
    };
    let parts: Vec<&str> = path.split('/').collect();
    let cur_oid = tree
        .peel_to_entry(parts.clone())
        .ok()
        .flatten()
        .map(|e| e.object_id());

    let parents: Vec<ObjectId> = commit.parent_ids().map(|id| id.detach()).collect();
    if parents.is_empty() {
        return cur_oid.is_some();
    }
    for parent_id in parents {
        let Ok(p_commit) = repo.find_commit(parent_id) else {
            continue;
        };
        let Ok(mut p_tree) = p_commit.tree() else {
            continue;
        };
        let p_oid = p_tree
            .peel_to_entry(parts.clone())
            .ok()
            .flatten()
            .map(|e| e.object_id());
        if cur_oid != p_oid {
            return true;
        }
    }
    false
}

/// Lists all local branches `(name, head_sha)` in the repository.
pub fn list_branches(repo_path: &Path) -> ShakoResult<Vec<(String, String)>> {
    let repo = open_isolated(repo_path)?;
    let platform = repo.references().map_err(git_err("references"))?;
    let iter = platform
        .local_branches()
        .map_err(git_err("local_branches"))?;
    let mut out = Vec::new();
    for mut r in iter.flatten() {
        let name = r
            .name()
            .as_bstr()
            .to_str_lossy()
            .strip_prefix("refs/heads/")
            .unwrap_or_default()
            .to_owned();
        if let Ok(id) = r.peel_to_id() {
            out.push((name, id.to_hex().to_string()));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Lists all tags `(name, tag_or_ref_oid, target_commit_sha, is_annotated)` in the repository (Row 33).
pub fn list_tags(repo_path: &Path) -> ShakoResult<Vec<(String, String, String, bool)>> {
    let repo = open_isolated(repo_path)?;
    let platform = repo.references().map_err(git_err("references"))?;
    let iter = platform.tags().map_err(git_err("tags"))?;
    let mut out = Vec::new();
    for mut r in iter.flatten() {
        let name = r
            .name()
            .as_bstr()
            .to_str_lossy()
            .strip_prefix("refs/tags/")
            .unwrap_or_default()
            .to_owned();
        let direct_id = r.target().try_id().map(|id| id.to_hex().to_string());
        let is_annotated = r
            .target()
            .try_id()
            .and_then(|id| repo.find_header(id).ok())
            .is_some_and(|h| h.kind() == gix_object::Kind::Tag);
        if let Ok(peeled) = r.peel_to_id() {
            let commit_sha = peeled.to_hex().to_string();
            let ref_oid = direct_id.unwrap_or_else(|| commit_sha.clone());
            out.push((name, ref_oid, commit_sha, is_annotated));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Creates a new branch `refs/heads/{new_branch}` pointing to the commit of `from_ref` (Row 24).
pub fn create_branch(repo_path: &Path, new_branch: &str, from_ref: &str) -> ShakoResult<String> {
    let full_name = branch_ref(new_branch)?;
    let repo = open_isolated(repo_path)?;
    let target_oid = resolve_commit_oid(&repo, from_ref)?;
    if repo
        .refs
        .try_find(full_name.as_bstr())
        .map_err(git_err("find ref"))?
        .is_some()
    {
        return Err(ShakoError::Conflict("The branch already exists.".into()));
    }
    repo.refs
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "create branch".into(),
                    },
                    expected: PreviousValue::MustNotExist,
                    new: Target::Object(target_oid),
                },
                name: full_name,
                deref: false,
            }],
            gix::lock::acquire::Fail::Immediately,
            gix::lock::acquire::Fail::Immediately,
        )
        .and_then(|tx| tx.commit(None))
        .map_err(|_| ShakoError::Conflict("The branch already exists.".into()))?;
    Ok(target_oid.to_hex().to_string())
}

/// Deletes `refs/heads/{branch}`, returning `Ok(false)` if it did not exist (Row 24).
pub fn delete_branch(repo_path: &Path, branch: &str) -> ShakoResult<bool> {
    let full_name = branch_ref(branch)?;
    let repo = open_isolated(repo_path)?;
    if repo
        .refs
        .try_find(full_name.as_bstr())
        .map_err(git_err("find ref"))?
        .is_none()
    {
        return Ok(false);
    }
    repo.refs
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Delete {
                    expected: PreviousValue::MustExist,
                    log: RefLog::AndReference,
                },
                name: full_name,
                deref: false,
            }],
            gix::lock::acquire::Fail::Immediately,
            gix::lock::acquire::Fail::Immediately,
        )
        .and_then(|tx| tx.commit(None))
        .map_err(git_err("delete branch"))?;
    Ok(true)
}

/// Sets a reference (such as `refs/pull/{number}/head`) directly to `oid_hex`.
pub fn set_ref_direct(repo_path: &Path, full_ref_str: &str, oid_hex: &str) -> ShakoResult<()> {
    let full_name = full_ref(full_ref_str)?;
    let repo = open_isolated(repo_path)?;
    let oid = ObjectId::from_hex(oid_hex.as_bytes())
        .map_err(|e| ShakoError::BadRequest(format!("invalid object id '{oid_hex}': {e}")))?;
    repo.refs
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "set ref".into(),
                    },
                    expected: PreviousValue::Any,
                    new: Target::Object(oid),
                },
                name: full_name,
                deref: false,
            }],
            gix::lock::acquire::Fail::Immediately,
            gix::lock::acquire::Fail::Immediately,
        )
        .and_then(|tx| tx.commit(None))
        .map_err(git_err("set ref"))?;
    Ok(())
}

/// Checks whether `ancestor` is an ancestor of (or equal to) `descendant`.
pub fn is_ancestor(repo: &gix::Repository, ancestor: ObjectId, descendant: ObjectId) -> bool {
    if ancestor == descendant {
        return true;
    }
    match repo.merge_base(ancestor, descendant) {
        Ok(Some(base)) => base.detach() == ancestor,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_of(repo: &gix::Repository, entries: &[(&str, EntryKind, ObjectId)]) -> ObjectId {
        let tree = gix_object::Tree {
            entries: entries
                .iter()
                .map(|(name, kind, oid)| gix_object::tree::Entry {
                    mode: (*kind).into(),
                    filename: (*name).into(),
                    oid: *oid,
                })
                .collect(),
        };
        repo.write_object(&tree).expect("write tree").detach()
    }

    #[test]
    fn a_repository_under_a_file_or_with_a_held_ref_lock_refuses_each_write() {
        use crate::testutil::{author, lock, put, repo};
        let tmp = tempfile::tempdir().expect("tmp");
        let file = tmp.path().join("plain");
        std::fs::write(&file, "x").expect("file");
        let under_file = init_bare_repo(
            &file,
            InstanceId::from_u128(1),
            RepoId::from_u128(1),
            "main",
            false,
            "",
        )
        .expect_err("root is a file");
        assert!(
            under_file
                .to_string()
                .contains("failed to create instance dir: "),
            "{under_file}"
        );

        let path = repo(tmp.path(), 1, true);
        put(&path, "main", "a.txt", b"a\n");
        lock(&path, "refs/heads/main");
        let change = FileChangeOp {
            operation: FileOpKind::Create,
            path: "b.txt".into(),
            content: Some(b"b\n".to_vec()),
            sha: None,
        };
        let commit = commit_file_changes(&path, "main", "add b", &author(), &author(), &[change])
            .expect_err("main is locked");
        assert!(
            commit
                .to_string()
                .contains("concurrent ref update on main: "),
            "{commit}"
        );
        assert_eq!(commit.status_code(), 409);
        let deleted = delete_branch(&path, "main").expect_err("main is locked");
        assert!(deleted.to_string().contains("delete branch: "), "{deleted}");

        lock(&path, "refs/heads/next");
        let created = create_branch(&path, "next", "main").expect_err("next is locked");
        assert_eq!(created.status_code(), 409);
        let tip = resolve_commit_oid(&open_isolated(&path).expect("open"), "main").expect("tip");
        let set =
            set_ref_direct(&path, "refs/heads/next", &tip.to_string()).expect_err("next is locked");
        assert!(set.to_string().contains("set ref: "), "{set}");
    }

    #[test]
    fn an_unreadable_ref_file_fails_branch_lookups_and_dangling_refs_are_not_listed() {
        use crate::testutil::{GONE, point, repo};
        let tmp = tempfile::tempdir().expect("tmp");
        let path = repo(tmp.path(), 1, true);
        point(&path, "refs/heads/broken", "not a ref");
        let created = create_branch(&path, "broken", "main").expect_err("unreadable ref");
        assert!(created.to_string().contains("find ref: "), "{created}");
        let deleted = delete_branch(&path, "broken").expect_err("unreadable ref");
        assert!(deleted.to_string().contains("find ref: "), "{deleted}");
        std::fs::remove_file(path.join("refs/heads/broken")).expect("rm");

        point(&path, "refs/heads/dangling", GONE);
        point(&path, "refs/tags/dangling", GONE);
        let branches = list_branches(&path).expect("branches");
        assert_eq!(
            branches.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["main"]
        );
        assert!(list_tags(&path).expect("tags").is_empty());
        let missing = read_commit_info(&path, GONE).expect_err("no such commit");
        assert_eq!(missing.status_code(), 404);
    }

    #[test]
    fn a_copy_leaves_out_a_symbolic_branch() {
        use crate::testutil::{point, repo};
        let tmp = tempfile::tempdir().expect("tmp");
        let origin = repo(tmp.path(), 1, true);
        point(&origin, "refs/heads/alias", "ref: refs/heads/main");
        let dest = repo(tmp.path(), 2, false);
        copy_bare_repo(&origin, &dest, "main").expect("copy");
        let names: Vec<String> = list_branches(&dest)
            .expect("branches")
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, ["main"]);
    }

    #[test]
    fn the_history_skips_commits_it_cannot_read_or_decode() {
        use crate::testutil::{GONE, commit, oid, point, raw, repo, tree};
        use gix_object::Kind;
        let tmp = tempfile::tempdir().expect("tmp");
        let path = repo(tmp.path(), 1, false);
        let blob = raw(&path, Kind::Blob, b"a\n");
        let files = tree(&path, &[(EntryKind::Blob, "a.txt", blob)]);
        let empty = tree(&path, &[]);

        // A parent that is gone: the child cannot compare, the parent is not walked.
        let orphan = commit(&path, files, &[oid(GONE)]);
        point(&path, "refs/heads/orphan", &orphan.to_string());
        assert!(
            list_commits(&path, "orphan", Some("a.txt"), 10, 100)
                .expect("log")
                .is_empty()
        );
        assert_eq!(
            list_commits(&path, "orphan", None, 10, 100)
                .expect("log")
                .len(),
            1
        );

        // A tree that is gone, in the commit itself and in its parent.
        let treeless = commit(&path, oid(GONE), &[]);
        let child = commit(&path, files, &[treeless]);
        point(&path, "refs/heads/treeless", &child.to_string());
        assert!(
            list_commits(&path, "treeless", Some("a.txt"), 10, 100)
                .expect("log")
                .is_empty()
        );

        // An author line that does not parse: the walk reads it, the log leaves it out.
        let sig = "t <t@example.org> 1700000000 +0000";
        let bad = format!("tree {empty}\nauthor {sig}\ncommitter {sig}\nbadheader\n\nbad\n");
        let undecodable = raw(&path, Kind::Commit, bad.as_bytes());
        point(&path, "refs/heads/bad", &undecodable.to_string());
        assert!(
            list_commits(&path, "bad", None, 10, 100)
                .expect("log")
                .is_empty()
        );
    }

    #[test]
    fn the_tree_walks_keep_links_and_fail_on_trees_they_cannot_read() {
        use crate::testutil::{GONE, oid, raw, repo, tree};
        use gix_object::Kind;
        let tmp = tempfile::tempdir().expect("tmp");
        let path = repo(tmp.path(), 1, false);
        let handle = open_isolated(&path).expect("open");
        let target = raw(&path, Kind::Blob, b"a.txt");
        let linked = tree(&path, &[(EntryKind::Link, "link", target)]);
        let mut blobs = BTreeMap::new();
        collect_tree_blobs(
            &handle,
            &handle.find_tree(linked).expect("tree"),
            "",
            &mut blobs,
        )
        .expect("walk");
        assert_eq!(blobs["link"], (EntryKind::Link, target));

        let garbage = raw(&path, Kind::Tree, b"not a tree");
        let new_dir = tree(&path, &[(EntryKind::Blob, "f", target)]);
        let new = tree(&path, &[(EntryKind::Tree, "d", new_dir)]);
        let new = handle.find_tree(new).expect("tree");
        let mut changed = BTreeMap::new();
        let unreadable_parent = handle.find_tree(garbage).expect("garbage tree");
        let err = collect_changed_blobs(&handle, &new, &[unreadable_parent], &mut changed)
            .expect_err("parent tree does not decode");
        assert!(err.to_string().contains("decode tree: "), "{err}");
        let lost = tree(&path, &[(EntryKind::Tree, "d", oid(GONE))]);
        let lost = handle.find_tree(lost).expect("tree");
        let err = collect_changed_blobs(&handle, &new, &[lost], &mut changed)
            .expect_err("parent subtree is gone");
        assert!(err.to_string().contains("find subtree: "), "{err}");
    }

    #[test]
    fn commit_dates_read_as_rfc3339_utc_as_chrono_wrote_them() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = gix::init_bare(tmp.path()).expect("init");
        let tree = tree_of(&repo, &[]);
        let sig = |seconds| gix::actor::Signature {
            name: "a".into(),
            email: "a@example.org".into(),
            time: gix::date::Time::new(seconds, 7200),
        };
        let commit = gix_object::Commit {
            tree,
            parents: Default::default(),
            author: sig(1_763_482_800),
            committer: sig(0),
            encoding: None,
            message: "m".into(),
            extra_headers: Vec::new(),
        };
        let id = repo.write_object(&commit).expect("write commit").detach();
        let info = read_commit_info(tmp.path(), &id.to_hex().to_string()).expect("read");
        // chrono's to_rfc3339(), pinned before the move to jiff (T-7237): UTC as `+00:00`
        // whatever the signature's own offset, whole seconds.
        assert_eq!(info.author_date, "2025-11-18T16:20:00+00:00");
        assert_eq!(info.committer_date, "1970-01-01T00:00:00+00:00");
    }

    #[test]
    fn a_tree_bomb_is_refused_before_it_fills_memory_or_the_stack() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = gix::init_bare(tmp.path()).expect("init");
        let blob = repo.write_blob(b"x").expect("blob").detach();

        // Fan-out: each level names the level below twice, 2^40 paths from 41 small trees.
        let mut level = tree_of(&repo, &[("f", EntryKind::Blob, blob)]);
        for _ in 0..40 {
            level = tree_of(
                &repo,
                &[("a", EntryKind::Tree, level), ("b", EntryKind::Tree, level)],
            );
        }
        let mut out = BTreeMap::new();
        let err = collect_tree_blobs(&repo, &repo.find_tree(level).unwrap(), "", &mut out)
            .expect_err("fan-out bomb");
        assert!(
            matches!(err, ShakoError::Unprocessable(ref m) if m.contains("entries")),
            "{err}"
        );

        // Depth: one long chain of nested trees.
        let mut chain = tree_of(&repo, &[("f", EntryKind::Blob, blob)]);
        for _ in 0..=MAX_TREE_DEPTH {
            chain = tree_of(&repo, &[("d", EntryKind::Tree, chain)]);
        }
        let mut out = BTreeMap::new();
        let err = collect_tree_blobs(&repo, &repo.find_tree(chain).unwrap(), "", &mut out)
            .expect_err("deep chain");
        assert!(
            matches!(err, ShakoError::Unprocessable(ref m) if m.contains("deeper")),
            "{err}"
        );

        // Length: a chain of long names makes every path below it long.
        let long = "n".repeat(250);
        let mut tall = tree_of(&repo, &[("f", EntryKind::Blob, blob)]);
        for _ in 0..20 {
            tall = tree_of(&repo, &[(long.as_str(), EntryKind::Tree, tall)]);
        }
        let mut out = BTreeMap::new();
        let err = collect_tree_blobs(&repo, &repo.find_tree(tall).unwrap(), "", &mut out)
            .expect_err("long paths");
        assert!(
            matches!(err, ShakoError::Unprocessable(ref m) if m.contains("bytes")),
            "{err}"
        );

        // The tree listing walks under the same limits.
        let mut listed = Vec::new();
        let walk = &mut TreeWalk::default();
        let err = walk_tree_entries(
            &repo,
            &repo.find_tree(level).unwrap(),
            "",
            true,
            0,
            walk,
            &mut listed,
        )
        .expect_err("listing a fan-out bomb");
        assert!(
            matches!(err, ShakoError::Unprocessable(ref m) if m.contains("entries")),
            "{err}"
        );

        // An ordinary tree still walks.
        let ok = tree_of(
            &repo,
            &[(
                "d",
                EntryKind::Tree,
                tree_of(&repo, &[("f", EntryKind::Blob, blob)]),
            )],
        );
        let mut out = BTreeMap::new();
        collect_tree_blobs(&repo, &repo.find_tree(ok).unwrap(), "", &mut out).expect("plain tree");
        assert_eq!(out.keys().collect::<Vec<_>>(), ["d/f"]);
    }

    #[test]
    fn a_submodule_entry_is_kept_as_a_gitlink_and_never_read_as_a_blob() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = gix::init_bare(tmp.path()).expect("init");
        let blob = repo.write_blob(b"x").expect("blob").detach();
        // The submodule's commit lives in another repository, so this one does not hold it.
        let elsewhere = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let tree = tree_of(
            &repo,
            &[
                ("f", EntryKind::Blob, blob),
                ("sub", EntryKind::Commit, elsewhere),
            ],
        );
        let tree = repo.find_tree(tree).unwrap();
        let mut all = BTreeMap::new();
        collect_tree_blobs(&repo, &tree, "", &mut all).expect("walk");
        assert_eq!(all.get("sub"), Some(&(EntryKind::Commit, elsewhere)));
        let mut changed = BTreeMap::new();
        collect_changed_blobs(&repo, &tree, &[], &mut changed).expect("walk changed");
        assert_eq!(changed.keys().collect::<Vec<_>>(), ["f"]);
    }

    use crate::testutil::{annotated_tag, author, oid, put, remove, repo};

    fn op(operation: FileOpKind, path: &str, content: &[u8], sha: Option<&str>) -> FileChangeOp {
        FileChangeOp {
            operation,
            path: path.into(),
            content: Some(content.to_vec()),
            sha: sha.map(str::to_owned),
        }
    }

    fn commit(path: &Path, branch: &str, ops: &[FileChangeOp]) -> ShakoResult<String> {
        commit_file_changes(path, branch, "m", &author(), &author(), ops)
    }

    fn blob_sha(path: &Path, branch: &str, file: &str) -> String {
        read_file_at_ref(path, branch, file, &mut crate::testutil::admit_all)
            .expect("read")
            .expect("present")
            .0
    }

    #[tokio::test]
    async fn one_repository_takes_one_writer_and_others_stay_free() {
        let locks = RepoLocks::new();
        let a = RepoId::from_u128(1);
        let b = RepoId::from_u128(2);
        let held = locks.lock(a).await;
        assert!(locks.try_lock(a).is_none(), "a second writer waits");
        let other = locks.try_lock(b).expect("another repository is free");
        drop(held);
        let again = locks.try_lock(a).expect("free once released");
        drop(again);
        drop(other);
        // A clone shares the registry.
        let shared = locks.clone();
        let _held = locks.lock(b).await;
        assert!(shared.try_lock(b).is_none());
    }

    #[test]
    fn a_directory_that_is_no_repository_does_not_open() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = open_isolated(tmp.path()).expect_err("not a repository");
        assert!(
            matches!(&err, ShakoError::Git(m) if m.contains("failed to open")),
            "{err}"
        );
    }

    #[test]
    fn init_writes_the_safe_config_and_head_and_refuses_a_bad_branch_or_a_taken_place() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let inst = InstanceId::from_u128(1);
        let bare =
            init_bare_repo(tmp.path(), inst, RepoId::from_u128(9), "dev", false, "").expect("init");
        assert_eq!(
            std::fs::read_to_string(bare.join("config")).expect("config"),
            SAFE_BARE_REPO_CONFIG
        );
        assert_eq!(
            std::fs::read_to_string(bare.join("description")).expect("d"),
            ""
        );
        let handle = open_isolated(&bare).expect("open");
        assert_eq!(
            handle
                .head_name()
                .expect("head")
                .expect("symbolic")
                .as_bstr(),
            "refs/heads/dev"
        );
        assert!(list_branches(&bare).expect("branches").is_empty(), "unborn");

        // An empty description still gives the README a line.
        let readme = init_bare_repo(tmp.path(), inst, RepoId::from_u128(10), "main", true, "")
            .expect("init");
        let (_, text) = read_file_at_ref(
            &readme,
            "main",
            "README.md",
            &mut crate::testutil::admit_all,
        )
        .expect("read")
        .expect("README");
        assert_eq!(text, b"# Repository\n\nInitialized by shako.\n");

        // The same repository twice, a bad branch name, an instance directory that is a file.
        let err = init_bare_repo(tmp.path(), inst, RepoId::from_u128(9), "dev", false, "")
            .expect_err("taken");
        assert!(matches!(err, ShakoError::Git(_)), "{err}");
        assert!(
            init_bare_repo(tmp.path(), inst, RepoId::from_u128(11), "a..b", false, "").is_err()
        );
        let file_root = tmp.path().join("plain-file");
        std::fs::write(&file_root, b"x").expect("file");
        let err = init_bare_repo(&file_root, inst, RepoId::from_u128(12), "main", false, "")
            .expect_err("instance dir under a file");
        assert!(matches!(err, ShakoError::Storage(_)), "{err}");
    }

    #[test]
    fn a_copy_takes_branches_tags_and_served_packs_and_leaves_the_rest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let origin = repo(tmp.path(), 1, true);
        let tip = put(&origin, "main", "a.txt", b"a\n");
        create_branch(&origin, "feature", "main").expect("branch");
        set_ref_direct(&origin, "refs/tags/v1", &tip.to_string()).expect("tag");
        set_ref_direct(&origin, "refs/pull/1/head", &tip.to_string()).expect("pull ref");
        let pack_dir = origin.join("objects/pack");
        // A superseded pack (no index), its cruft times and an unrelated file.
        std::fs::write(pack_dir.join("pack-old.pack"), b"PACK").expect("superseded");
        std::fs::write(pack_dir.join("pack-old.seen"), b"").expect("seen");
        std::fs::write(pack_dir.join("notes.txt"), b"x").expect("other");

        let dest = repo(tmp.path(), 2, false);
        copy_bare_repo(&origin, &dest, "feature").expect("copy");
        let copied = dest.join("objects/pack");
        assert!(!copied.join("pack-old.pack").exists());
        assert!(copied.join("pack-old.seen").exists());
        assert!(!copied.join("notes.txt").exists());
        let branches = list_branches(&dest).expect("branches");
        assert_eq!(
            branches.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["feature", "main"]
        );
        assert_eq!(list_tags(&dest).expect("tags")[0].0, "v1");
        let handle = open_isolated(&dest).expect("open");
        assert!(handle.find_reference("refs/pull/1/head").is_err());
        assert_eq!(
            handle.head_name().expect("head").expect("sym").as_bstr(),
            "refs/heads/feature"
        );
        assert_eq!(resolve_commit_oid(&handle, "HEAD").expect("head"), tip);

        // No origin, or a default branch no ref name takes.
        let missing = tmp.path().join("missing.git");
        assert!(copy_bare_repo(&missing, &dest, "main").is_err());
        let fresh = repo(tmp.path(), 3, false);
        assert!(copy_bare_repo(&origin, &fresh, "bad..name").is_err());
        // An origin without a pack directory copies its refs alone.
        let empty = repo(tmp.path(), 4, false);
        std::fs::remove_dir_all(empty.join("objects/pack")).expect("drop pack dir");
        let into = repo(tmp.path(), 5, false);
        copy_bare_repo(&empty, &into, "main").expect("empty copy");
        assert!(list_branches(&into).expect("branches").is_empty());
    }

    #[test]
    fn the_size_counts_every_object_file_and_is_zero_without_objects() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(compute_repo_size_bytes(&tmp.path().join("none")), 0);
        let path = repo(tmp.path(), 1, false);
        let empty = compute_repo_size_bytes(&path);
        put(&path, "main", "a.txt", &[7u8; 4096]);
        let grown = compute_repo_size_bytes(&path);
        assert!(grown > empty, "{empty} -> {grown}");
        std::fs::create_dir_all(path.join("objects/ab")).expect("dir");
        std::fs::write(path.join("objects/ab/cd"), [0u8; 100]).expect("loose");
        assert_eq!(compute_repo_size_bytes(&path), grown + 100);
    }

    #[test]
    fn writing_no_objects_writes_no_pack() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, false);
        let before = std::fs::read_dir(path.join("objects/pack"))
            .expect("dir")
            .count();
        write_objects_as_pack(&path, &[]).expect("nothing to write");
        let after = std::fs::read_dir(path.join("objects/pack"))
            .expect("dir")
            .count();
        assert_eq!(before, after);
    }

    #[test]
    fn a_ref_resolves_by_sha_branch_tag_pull_ref_or_revision_and_nothing_else() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        let first = put(&path, "main", "a.txt", b"1\n");
        let second = put(&path, "main", "a.txt", b"2\n");
        set_ref_direct(&path, "refs/tags/light", &first.to_string()).expect("tag");
        annotated_tag(&path, "heavy", first, gix_object::Kind::Commit);
        set_ref_direct(&path, "refs/pull/7/head", &first.to_string()).expect("pull");
        let handle = open_isolated(&path).expect("open");
        let resolve = |r: &str| resolve_commit_oid(&handle, r);
        assert_eq!(resolve(&second.to_string()).expect("sha"), second);
        assert_eq!(resolve(&format!(" {second} ")).expect("trimmed"), second);
        assert_eq!(resolve("main").expect("branch"), second);
        assert_eq!(resolve("refs/heads/main").expect("full"), second);
        assert_eq!(resolve("light").expect("tag"), first);
        assert_eq!(resolve("heavy").expect("annotated"), first);
        assert_eq!(resolve("7/head").expect("pull"), first);
        assert_eq!(resolve("main~1").expect("revision"), first);
        for unknown in [
            "nope",
            "1111111111111111111111111111111111111111",
            "",
            "main~9",
        ] {
            let err = resolve(unknown).expect_err("unknown");
            assert!(matches!(err, ShakoError::NotFound(_)), "{unknown}: {err}");
        }
        // A blob's id is no commit.
        let blob = blob_sha(&path, "main", "a.txt");
        assert!(matches!(resolve(&blob), Err(ShakoError::NotFound(_))));
    }

    #[test]
    fn file_changes_check_every_precondition_before_anything_is_written() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        let tip = || resolve_commit_oid(&open_isolated(&path).expect("open"), "main").expect("tip");
        let start = tip();
        let readme = blob_sha(&path, "main", "README.md");
        let wrong = "1111111111111111111111111111111111111111";
        let cases: Vec<(Vec<FileChangeOp>, u16, &str)> = vec![
            (vec![], 422, "no file changes"),
            (
                vec![op(FileOpKind::Create, "README.md", b"x", None)],
                422,
                "already exists",
            ),
            (
                vec![op(FileOpKind::Update, "none.txt", b"x", Some(wrong))],
                404,
                "does not exist",
            ),
            (
                vec![op(FileOpKind::Update, "README.md", b"x", None)],
                422,
                "requires blob sha",
            ),
            (
                vec![op(FileOpKind::Update, "README.md", b"x", Some(""))],
                422,
                "requires blob sha",
            ),
            (
                vec![op(FileOpKind::Update, "README.md", b"x", Some(wrong))],
                409,
                "sha mismatch",
            ),
            (
                vec![op(FileOpKind::Upload, "none.txt", b"x", Some(wrong))],
                409,
                "does not exist",
            ),
            (
                vec![op(FileOpKind::Upload, "README.md", b"x", Some(wrong))],
                409,
                "sha mismatch",
            ),
            (
                vec![op(FileOpKind::Delete, "none.txt", b"", Some(wrong))],
                404,
                "does not exist",
            ),
            (
                vec![op(FileOpKind::Delete, "README.md", b"", None)],
                422,
                "requires blob sha",
            ),
            (
                vec![op(FileOpKind::Delete, "README.md", b"", Some(wrong))],
                409,
                "sha mismatch",
            ),
            // The first change is fine; the second fails, so neither lands.
            (
                vec![
                    op(FileOpKind::Create, "new.txt", b"x", None),
                    op(FileOpKind::Create, "../escape", b"x", None),
                ],
                0,
                "",
            ),
        ];
        for (ops, status, says) in cases {
            let err = commit(&path, "main", &ops).expect_err("refused");
            if status != 0 {
                assert_eq!(err.status_code(), status, "{err}");
                assert!(err.to_string().contains(says), "{says}: {err}");
            }
            assert_eq!(tip(), start, "nothing written for {err}");
        }
        assert!(
            commit(
                &path,
                "bad..branch",
                &[op(FileOpKind::Create, "x", b"", None)]
            )
            .is_err()
        );

        // Each kind that holds: update, upload over and beside, delete, in nested directories.
        commit(
            &path,
            "main",
            &[op(FileOpKind::Update, "README.md", b"new\n", Some(&readme))],
        )
        .expect("update");
        let readme = blob_sha(&path, "main", "README.md");
        commit(
            &path,
            "main",
            &[
                op(FileOpKind::Upload, "README.md", b"up\n", Some(&readme)),
                op(FileOpKind::Upload, "a/b/c.txt", b"deep\n", Some("")),
                FileChangeOp {
                    operation: FileOpKind::Create,
                    path: "a/empty.txt".into(),
                    content: None,
                    sha: None,
                },
            ],
        )
        .expect("upload");
        let (_, entries) = list_tree_entries(&path, "main", true).expect("tree");
        let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["README.md", "a", "a/b", "a/b/c.txt", "a/empty.txt"]);
        remove(&path, "main", "a/b/c.txt");
        let (_, entries) = list_tree_entries(&path, "main", true).expect("tree");
        assert!(
            entries.iter().all(|e| !e.path.starts_with("a/b")),
            "{entries:?}"
        );

        // A branch that does not exist yet starts with a root commit.
        let root = commit(
            &path,
            "orphan",
            &[op(FileOpKind::Create, "o.txt", b"o", None)],
        )
        .expect("root commit");
        assert!(
            read_commit_info(&path, &root)
                .expect("info")
                .parent_shas
                .is_empty()
        );
    }

    #[test]
    fn an_actor_field_loses_what_would_break_the_header_and_falls_back_when_empty() {
        assert_eq!(sanitize_actor_field(" Al<i>ce\n ", "x"), "Alice");
        assert_eq!(sanitize_actor_field("<>\r\0", "fallback"), "fallback");
        assert_eq!(sanitize_actor_field("", "fallback"), "fallback");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, false);
        let nobody = CommitAuthor {
            name: "<>".into(),
            email: "\n".into(),
        };
        let sha = commit_file_changes(
            &path,
            "main",
            "m",
            &nobody,
            &nobody,
            &[op(FileOpKind::Create, "a", b"", None)],
        )
        .expect("commit");
        let info = read_commit_info(&path, &sha).expect("info");
        assert_eq!(
            (info.author_name.as_str(), info.author_email.as_str()),
            ("shako", "shako@local")
        );
        assert_eq!(info.committer_name, "shako");
    }

    #[test]
    fn tree_order_compares_a_directory_as_if_it_ended_in_a_slash() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        assert_eq!(tree_entry_cmp(b"a", true, b"a.b", false), Greater);
        assert_eq!(tree_entry_cmp(b"a", false, b"a.b", false), Less);
        assert_eq!(tree_entry_cmp(b"a", true, b"a0", false), Less);
        assert_eq!(tree_entry_cmp(b"b", false, b"a", false), Greater);
        assert_eq!(tree_entry_cmp(b"a", false, b"a", false), Equal);
        assert_eq!(tree_entry_cmp(b"a", true, b"a", false), Greater);
    }

    #[test]
    fn a_tree_lists_by_ref_or_tree_sha_with_each_mode_and_size() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        let handle = open_isolated(&path).expect("open");
        let blob = handle.write_blob(b"12345").expect("blob").detach();
        let sub = tree_of(&handle, &[("f", EntryKind::Blob, blob)]);
        let tree = tree_of(
            &handle,
            &[
                ("d", EntryKind::Tree, sub),
                ("link", EntryKind::Link, blob),
                ("run.sh", EntryKind::BlobExecutable, blob),
            ],
        );
        let (sha, top) = list_tree_entries(&path, &tree.to_string(), false).expect("by tree sha");
        assert_eq!(sha, tree.to_string());
        let modes: Vec<_> = top
            .iter()
            .map(|e| {
                (
                    e.path.as_str(),
                    e.mode.as_str(),
                    e.entry_type.as_str(),
                    e.size,
                )
            })
            .collect();
        assert_eq!(
            modes,
            [
                ("d", "040000", "tree", 0),
                ("link", "120000", "blob", 5),
                ("run.sh", "100755", "blob", 5)
            ]
        );
        let (_, all) = list_tree_entries(&path, &tree.to_string(), true).expect("recursive");
        assert_eq!(all[1].path, "d/f");
        assert_eq!(all[1].mode, "100644");
        // A commit's sha lists its tree; an unknown ref is not found.
        let (_, readme) = list_tree_entries(&path, "main", false).expect("by branch");
        assert_eq!(readme[0].path, "README.md");
        assert!(matches!(
            list_tree_entries(&path, "nope", false),
            Err(ShakoError::NotFound(_))
        ));
    }

    #[test]
    fn a_file_reads_with_its_sha_and_a_directory_or_absent_path_reads_as_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        put(&path, "main", "dir/f.txt", b"body\n");
        let mut admitted = 0;
        let (sha, bytes) = read_file_at_ref(&path, "main", "dir/f.txt", &mut |n| {
            admitted = n;
            Ok(())
        })
        .expect("read")
        .expect("present");
        assert_eq!(bytes, b"body\n");
        assert_eq!(sha.len(), 40);
        assert_eq!(admitted, 15, "three times the blob");
        let none = |file: &str| {
            read_file_at_ref(&path, "main", file, &mut crate::testutil::admit_all).expect("read")
        };
        assert_eq!(none("dir"), None);
        assert_eq!(none("missing.txt"), None);
        assert_eq!(none("dir/missing/deeper"), None);
        let refused = read_file_at_ref(&path, "main", "dir/f.txt", &mut |_| {
            Err(ShakoError::Overloaded("busy".into()))
        });
        assert!(matches!(refused, Err(ShakoError::Overloaded(_))));
        assert!(
            read_file_at_ref(
                &path,
                "main",
                "../etc/passwd",
                &mut crate::testutil::admit_all
            )
            .is_err()
        );
        assert!(matches!(
            read_file_at_ref(&path, "nope", "dir/f.txt", &mut crate::testutil::admit_all),
            Err(ShakoError::NotFound(_))
        ));
    }

    #[test]
    fn history_walks_newest_first_within_limits_and_by_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        put(&path, "main", "a.txt", b"1\n");
        put(&path, "main", "b.txt", b"1\n");
        put(&path, "main", "a.txt", b"2\n");
        let all = list_commits(&path, "main", None, 50, 10_000).expect("all");
        // The commits share one second, so the order among them is the walk's own.
        let mut messages: Vec<_> = all.iter().map(|c| c.message.as_str()).collect();
        messages.sort_unstable();
        assert_eq!(messages, ["Initial commit", "a.txt", "a.txt", "b.txt"]);
        assert_eq!(
            list_commits(&path, "main", None, 2, 10_000)
                .expect("limit")
                .len(),
            2
        );
        assert_eq!(
            list_commits(&path, "main", None, 50, 1)
                .expect("walk cap")
                .len(),
            1
        );
        let touching = list_commits(&path, "main", Some("/a.txt/"), 50, 10_000).expect("by path");
        assert_eq!(touching.len(), 2);
        let readme = list_commits(&path, "main", Some("README.md"), 50, 10_000).expect("root");
        assert_eq!(readme.len(), 1, "the root commit adds it");
        assert_eq!(
            list_commits(&path, "main", Some("/"), 50, 10_000)
                .expect("no filter")
                .len(),
            4
        );
        assert!(list_commits(&path, "main", Some("../x"), 50, 10_000).is_err());
        assert!(matches!(
            list_commits(&path, "nope", None, 50, 10_000),
            Err(ShakoError::NotFound(_))
        ));

        // A merge commit touches a path when it differs from either parent.
        create_branch(&path, "side", "main~1").expect("side");
        put(&path, "side", "c.txt", b"side\n");
        crate::merge::execute_pr_merge(
            &path,
            "main",
            "side",
            &crate::merge::MergeOptions {
                expected_head_sha: None,
                style: crate::merge::MergeStyle::Merge,
                message: "merge side",
                author: &author(),
                committer: &author(),
            },
        )
        .expect("merge");
        let by_c = list_commits(&path, "main", Some("c.txt"), 50, 10_000).expect("c");
        let mut messages: Vec<_> = by_c.iter().map(|c| c.message.as_str()).collect();
        messages.sort_unstable();
        assert_eq!(messages, ["c.txt", "merge side"]);
        let by_b = list_commits(&path, "main", Some("b.txt"), 50, 10_000).expect("b");
        assert_eq!(by_b.len(), 1, "the merge holds b.txt as main did");
    }

    #[test]
    fn branches_and_tags_list_sorted_with_each_tag_kind() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        let tip = put(&path, "main", "a.txt", b"1\n");
        create_branch(&path, "b-two", "main").expect("branch");
        create_branch(&path, "a-one", "main").expect("branch");
        let names: Vec<_> = list_branches(&path)
            .expect("branches")
            .into_iter()
            .map(|b| b.0)
            .collect();
        assert_eq!(names, ["a-one", "b-two", "main"]);
        set_ref_direct(&path, "refs/tags/z-light", &tip.to_string()).expect("tag");
        let heavy = annotated_tag(&path, "a-heavy", tip, gix_object::Kind::Commit);
        let tags = list_tags(&path).expect("tags");
        assert_eq!(
            tags,
            [
                (
                    String::from("a-heavy"),
                    heavy.to_string(),
                    tip.to_string(),
                    true
                ),
                (
                    String::from("z-light"),
                    tip.to_string(),
                    tip.to_string(),
                    false
                ),
            ]
        );
        assert!(
            list_tags(&repo(tmp.path(), 2, false))
                .expect("none")
                .is_empty()
        );
    }

    #[test]
    fn a_branch_is_created_once_from_a_known_ref_and_deleted_once() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        let tip = resolve_commit_oid(&open_isolated(&path).expect("open"), "main").expect("tip");
        assert_eq!(
            create_branch(&path, "f", "main").expect("create"),
            tip.to_string()
        );
        let err = create_branch(&path, "f", "main").expect_err("exists");
        assert_eq!(err.status_code(), 409);
        assert!(create_branch(&path, "bad..name", "main").is_err());
        assert!(matches!(
            create_branch(&path, "g", "nope"),
            Err(ShakoError::NotFound(_))
        ));
        assert!(delete_branch(&path, "f").expect("delete"));
        assert!(!delete_branch(&path, "f").expect("gone"));
        assert!(delete_branch(&path, "bad..name").is_err());
        assert_eq!(list_branches(&path).expect("branches").len(), 1);
    }

    #[test]
    fn a_ref_is_set_only_to_a_valid_id_under_a_valid_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        let tip = resolve_commit_oid(&open_isolated(&path).expect("open"), "main").expect("tip");
        set_ref_direct(&path, "refs/pull/1/head", &tip.to_string()).expect("set");
        set_ref_direct(&path, "refs/pull/1/head", &tip.to_string()).expect("again, unchanged");
        let err = set_ref_direct(&path, "refs/pull/1/head", "xyz").expect_err("bad id");
        assert_eq!(err.status_code(), 400);
        assert!(set_ref_direct(&path, "refs/heads/a..b", &tip.to_string()).is_err());
    }

    #[test]
    fn ancestry_holds_for_the_same_commit_and_its_parents_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        let first = resolve_commit_oid(&open_isolated(&path).expect("open"), "main").expect("tip");
        let second = put(&path, "main", "a.txt", b"1\n");
        let orphan = put(&path, "orphan", "o.txt", b"o\n");
        let handle = open_isolated(&path).expect("open");
        assert!(is_ancestor(&handle, second, second));
        assert!(is_ancestor(&handle, first, second));
        assert!(!is_ancestor(&handle, second, first));
        assert!(!is_ancestor(&handle, orphan, second), "no common history");
        assert!(!is_ancestor(
            &handle,
            oid("1111111111111111111111111111111111111111"),
            second
        ));
    }
}
