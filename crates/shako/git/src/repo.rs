#![forbid(unsafe_code)]
//! Isolated bare Git repository lifecycle, per-repository write locks, tree/commit/file reads,
//! and pack-backed API commits (S2, S11, R§2.4, R§2.5, Threat T1, T2).

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
use tokio::sync::{OwnedMutexGuard, Mutex as AsyncMutex};

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
    opts = opts.open_path_as_is(true);
    let mut repo = gix::open_opts(path, opts)
        .map_err(|e| ShakoError::Git(format!("failed to open repository: {e}")))?;
    // Cap decoded object cache at 2 MiB per repository handle (S22)
    repo.object_cache_size(Some(2 * 1024 * 1024));
    Ok(repo)
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
    validate_branch_name(default_branch)?;
    let path = repo_disk_path(root, instance_id, repo_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ShakoError::Storage(format!("failed to create instance dir: {e}")))?;
    }
    let repo = gix::init_bare(&path)
        .map_err(|e| ShakoError::Git(format!("gix::init_bare failed: {e}")))?;
    std::fs::write(path.join("config"), SAFE_BARE_REPO_CONFIG)
        .map_err(|e| ShakoError::Storage(format!("failed to write repo config: {e}")))?;
    std::fs::write(path.join("description"), description)
        .map_err(|e| ShakoError::Storage(format!("failed to write repo description: {e}")))?;

    let head_target: FullName = format!("refs/heads/{default_branch}")
        .try_into()
        .map_err(|e| ShakoError::Unprocessable(format!("invalid default branch: {e}")))?;
    let head_name: FullName = "HEAD"
        .try_into()
        .map_err(|e| ShakoError::Internal(format!("HEAD name: {e}")))?;
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
        .map_err(|e| ShakoError::Git(format!("prepare HEAD failed: {e}")))?
        .commit(None)
        .map_err(|e| ShakoError::Git(format!("commit HEAD failed: {e}")))?;

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
    std::fs::create_dir_all(&dest_pack_dir)
        .map_err(|e| ShakoError::Storage(format!("mkdir pack dir: {e}")))?;
    if origin_pack_dir.is_dir() {
        for entry in std::fs::read_dir(&origin_pack_dir)
            .map_err(|e| ShakoError::Storage(e.to_string()))?
            .flatten()
        {
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            if name_str.ends_with(".pack") || name_str.ends_with(".idx") {
                std::fs::copy(entry.path(), dest_pack_dir.join(&file_name))
                    .map_err(|e| ShakoError::Storage(format!("copy pack: {e}")))?;
            }
        }
    }

    // Also pack any loose objects from origin
    let mut edits = Vec::new();
    let platform = origin
        .references()
        .map_err(|e| ShakoError::Git(format!("references: {e}")))?;
    let all_refs = platform
        .all()
        .map_err(|e| ShakoError::Git(format!("all refs: {e}")))?;
    for r in all_refs.flatten() {
        let name_str = r.name().as_bstr().to_str_lossy().into_owned();
        if !(name_str.starts_with("refs/heads/") || name_str.starts_with("refs/tags/")) {
            continue;
        }
        if let Some(id) = r.try_id() {
            let full_name: FullName = name_str
                .try_into()
                .map_err(|e| ShakoError::Git(format!("ref name: {e}")))?;
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

    let head_target: FullName = format!("refs/heads/{default_branch}")
        .try_into()
        .map_err(|e| ShakoError::Git(format!("default branch: {e}")))?;
    let head_name: FullName = "HEAD"
        .try_into()
        .map_err(|e| ShakoError::Git(format!("HEAD: {e}")))?;
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
        .map_err(|e| ShakoError::Git(format!("migrate ref prepare: {e}")))?
        .commit(None)
        .map_err(|e| ShakoError::Git(format!("migrate ref commit: {e}")))?;

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
        for entry in rd.flatten() {
            let path = entry.path();
            if let Ok(meta) = entry.metadata() {
                if meta.is_dir() {
                    sum = sum.saturating_add(dir_bytes(&path));
                } else {
                    sum = sum.saturating_add(meta.len());
                }
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
            .map_err(|e| ShakoError::Git(format!("pack entry deflate failed: {e}")))?;
        entries.push(entry);
    }

    let mut pack_bytes = Vec::new();
    let num_entries = u32::try_from(entries.len())
        .map_err(|_| ShakoError::BadRequest("too many objects in commit".into()))?;
    let mut iter = output::bytes::FromEntriesIter::new(
        std::iter::once(gix::Result::Ok(entries)),
        &mut pack_bytes,
        num_entries,
        gix_pack::data::Version::V2,
        gix_hash::Kind::Sha1,
    );
    while let Some(res) = iter.next() {
        res.map_err(|e| ShakoError::Git(format!("pack encode failed: {e}")))?;
    }

    let pack_dir = repo_path.join("objects/pack");
    std::fs::create_dir_all(&pack_dir)
        .map_err(|e| ShakoError::Storage(format!("mkdir pack dir: {e}")))?;
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
    .map_err(|e| ShakoError::Git(format!("write pack bundle failed: {e}")))?;
    if let Some(keep) = outcome.keep_path {
        let _ = std::fs::remove_file(keep);
    }
    Ok(())
}

fn hash_and_serialize_object(
    kind: gix_object::Kind,
    obj: &impl WriteTo,
) -> ShakoResult<(ObjectId, Vec<u8>)> {
    let mut raw = Vec::new();
    obj.write_to(&mut raw)
        .map_err(|e| ShakoError::Git(format!("serialize object: {e}")))?;
    let oid = gix_object::compute_hash(gix_hash::Kind::Sha1, kind, &raw)
        .map_err(|e| ShakoError::Git(format!("compute hash: {e}")))?;
    Ok((oid, raw))
}

fn hash_blob_bytes(bytes: &[u8]) -> ShakoResult<ObjectId> {
    gix_object::compute_hash(gix_hash::Kind::Sha1, gix_object::Kind::Blob, bytes)
        .map_err(|e| ShakoError::Git(format!("compute blob hash: {e}")))
}

/// Resolves a ref/branch/tag/SHA string to its peeled commit `ObjectId`.
pub fn resolve_commit_oid(repo: &gix::Repository, git_ref: &str) -> ShakoResult<ObjectId> {
    let trimmed = git_ref.trim();
    if trimmed.len() == 40 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        if let Ok(oid) = ObjectId::from_hex(trimmed.as_bytes()) {
            if repo.find_commit(oid).is_ok() {
                return Ok(oid);
            }
        }
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
        if let Ok(Some(mut r)) = repo.try_find_reference(partial) {
            if let Ok(commit) = r.peel_to_commit() {
                return Ok(commit.id);
            }
        }
    }
    if let Ok(id) = repo.rev_parse_single(trimmed) {
        if let Ok(obj) = id.object()
            && let Ok(commit) = obj.peel_to_kind(gix_object::Kind::Commit)
        {
            return Ok(commit.id);
        }
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
    validate_branch_name(branch)?;
    if changes.is_empty() {
        return Err(ShakoError::Unprocessable("no file changes specified".into()));
    }
    let repo = open_isolated(repo_path)?;
    let full_ref_str = format!("refs/heads/{branch}");
    let full_ref: FullName = full_ref_str
        .clone()
        .try_into()
        .map_err(|e| ShakoError::Unprocessable(format!("invalid branch: {e}")))?;

    let parent_commit_oid = match repo.refs.try_find(full_ref.as_bstr()) {
        Ok(Some(r)) => r.target.try_id().map(ToOwned::to_owned),
        _ => None,
    };

    // Load flat path -> (EntryKind, ObjectId) map from parent commit's tree
    let mut tree_entries: BTreeMap<String, (EntryKind, ObjectId)> = BTreeMap::new();
    if let Some(parent_oid) = parent_commit_oid {
        let commit = repo
            .find_commit(parent_oid)
            .map_err(|e| ShakoError::Git(format!("find parent commit: {e}")))?;
        let tree = commit
            .tree()
            .map_err(|e| ShakoError::Git(format!("find parent tree: {e}")))?;
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
                if cur_oid.to_hex().to_string() != expected_sha {
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
                    if cur_oid.to_hex().to_string() != expected_sha {
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
                if cur_oid.to_hex().to_string() != expected_sha {
                    return Err(ShakoError::Conflict(format!(
                        "file '{clean_path}' sha mismatch: expected {expected_sha}, found {cur_oid}"
                    )));
                }
                tree_entries.remove(&clean_path);
            }
        }
    }

    let root_tree_oid = build_nested_trees(&tree_entries, &mut new_objects)?;
    let now_secs = chrono::Utc::now().timestamp();
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
        .map_err(|e| ShakoError::Conflict(format!("concurrent ref update on {branch}: {e}")))?
        .commit(None)
        .map_err(|e| ShakoError::Conflict(format!("failed to commit ref {branch}: {e}")))?;

    Ok(commit_oid.to_hex().to_string())
}

fn sanitize_actor_field(s: &str, fallback: &str) -> String {
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

pub(crate) fn collect_tree_blobs(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &str,
    out: &mut BTreeMap<String, (EntryKind, ObjectId)>,
) -> ShakoResult<()> {
    let decoded = tree
        .decode()
        .map_err(|e| ShakoError::Git(format!("decode tree: {e}")))?;
    for entry in decoded.entries {
        let name = entry.filename.to_str_lossy();
        let full = if prefix.is_empty() {
            name.into_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if entry.mode.is_tree() {
            let sub = repo
                .find_tree(entry.oid)
                .map_err(|e| ShakoError::Git(format!("find subtree: {e}")))?;
            collect_tree_blobs(repo, &sub, &full, out)?;
        } else {
            let kind = if entry.mode.is_executable() {
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

    fn insert_path(node: &mut DirNode, parts: &[&str], kind: EntryKind, oid: ObjectId) {
        match parts {
            [] => {}
            [leaf] => {
                node.files.insert((*leaf).to_owned(), (kind, oid));
            }
            [first, rest @ ..] => {
                let child = node.subdirs.entry((*first).to_owned()).or_default();
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
            tree_entry_cmp(a.filename.as_slice(), a_is_tree, b.filename.as_slice(), b_is_tree)
        });
        let tree_obj = gix_object::Tree { entries };
        let (oid, raw) = hash_and_serialize_object(gix_object::Kind::Tree, &tree_obj)?;
        new_objects.push((oid, gix_object::Kind::Tree, raw));
        Ok(oid)
    }

    let mut root = DirNode::default();
    for (path, (kind, oid)) in files {
        let parts: Vec<&str> = path.split('/').collect();
        insert_path(&mut root, &parts, *kind, *oid);
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
            .map_err(|e| ShakoError::Git(format!("find commit: {e}")))?;
        commit
            .tree()
            .map_err(|e| ShakoError::Git(format!("find tree: {e}")))?
    };

    let tree_sha = tree.id.to_hex().to_string();
    let mut out = Vec::new();
    walk_tree_entries(&repo, &tree, "", recursive, &mut out)?;
    Ok((tree_sha, out))
}

fn walk_tree_entries(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &str,
    recursive: bool,
    out: &mut Vec<TreeEntryInfo>,
) -> ShakoResult<()> {
    let decoded = tree
        .decode()
        .map_err(|e| ShakoError::Git(format!("decode tree: {e}")))?;
    for entry in decoded.entries {
        let name = entry.filename.to_str_lossy();
        let path = if prefix.is_empty() {
            name.into_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let is_tree = entry.mode.is_tree();
        let (mode_str, type_str, size) = if is_tree {
            ("040000".to_owned(), "tree".to_owned(), 0)
        } else {
            let sz = repo
                .find_header(entry.oid)
                .map(|h| h.size())
                .unwrap_or(0);
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
            let sub = repo
                .find_tree(entry.oid)
                .map_err(|e| ShakoError::Git(format!("find subtree: {e}")))?;
            walk_tree_entries(repo, &sub, &path, true, out)?;
        }
    }
    Ok(())
}

/// Reads a single file's bytes and blob SHA at `git_ref` (Row 25: `GET contents/{path}?ref=`).
pub fn read_file_at_ref(
    repo_path: &Path,
    git_ref: &str,
    file_path: &str,
) -> ShakoResult<Option<(String, Vec<u8>)>> {
    let clean = validate_tree_path(file_path)?;
    let repo = open_isolated(repo_path)?;
    let commit_oid = resolve_commit_oid(&repo, git_ref)?;
    let commit = repo
        .find_commit(commit_oid)
        .map_err(|e| ShakoError::Git(format!("find commit: {e}")))?;
    let mut tree = commit
        .tree()
        .map_err(|e| ShakoError::Git(format!("find tree: {e}")))?;
    let parts: Vec<&str> = clean.split('/').collect();
    let Some(entry) = tree
        .peel_to_entry(parts)
        .map_err(|e| ShakoError::Git(format!("lookup entry: {e}")))?
    else {
        return Ok(None);
    };
    if entry.mode().is_tree() {
        return Ok(None);
    }
    let blob = repo
        .find_blob(entry.oid())
        .map_err(|e| ShakoError::Git(format!("find blob: {e}")))?;
    Ok(Some((blob.id.to_hex().to_string(), blob.data.clone())))
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
    let commit = repo
        .find_commit(oid)
        .map_err(|e| ShakoError::NotFound(format!("commit '{sha_or_ref}' not found: {e}")))?;
    decode_commit_info(&commit)
}

fn decode_commit_info(commit: &gix::Commit<'_>) -> ShakoResult<CommitInfo> {
    let decoded = commit
        .decode()
        .map_err(|e| ShakoError::Git(format!("decode commit: {e}")))?;
    let author = decoded.author().unwrap_or_default();
    let committer = decoded.committer().unwrap_or_default();
    let author_dt = chrono::DateTime::from_timestamp(author.time().map(|t| t.seconds).unwrap_or(0), 0)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339();
    let committer_dt = chrono::DateTime::from_timestamp(committer.time().map(|t| t.seconds).unwrap_or(0), 0)
        .unwrap_or_else(chrono::Utc::now)
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
    let filter_path = match path_filter.map(|p| p.trim_matches('/')).filter(|p| !p.is_empty()) {
        Some(p) => Some(validate_tree_path(p)?),
        None => None,
    };

    let walk = repo
        .rev_walk([start_oid])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()
        .map_err(|e| ShakoError::Git(format!("rev_walk failed: {e}")))?;

    let mut out = Vec::new();
    for (walked, item) in walk.enumerate() {
        if walked >= max_walk || out.len() >= limit {
            break;
        }
        let Ok(info) = item else {
            continue;
        };
        let Ok(commit) = info.object() else {
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
    let platform = repo
        .references()
        .map_err(|e| ShakoError::Git(format!("references: {e}")))?;
    let iter = platform
        .local_branches()
        .map_err(|e| ShakoError::Git(format!("local_branches: {e}")))?;
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
    let platform = repo
        .references()
        .map_err(|e| ShakoError::Git(format!("references: {e}")))?;
    let iter = platform
        .tags()
        .map_err(|e| ShakoError::Git(format!("tags: {e}")))?;
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
    validate_branch_name(new_branch)?;
    let repo = open_isolated(repo_path)?;
    let target_oid = resolve_commit_oid(&repo, from_ref)?;
    let full_name: FullName = format!("refs/heads/{new_branch}")
        .try_into()
        .map_err(|e| ShakoError::Unprocessable(format!("invalid branch name: {e}")))?;
    if repo
        .refs
        .try_find(full_name.as_bstr())
        .map_err(|e| ShakoError::Git(e.to_string()))?
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
        .map_err(|_| ShakoError::Conflict("The branch already exists.".into()))?
        .commit(None)
        .map_err(|_| ShakoError::Conflict("The branch already exists.".into()))?;
    Ok(target_oid.to_hex().to_string())
}

/// Deletes `refs/heads/{branch}`, returning `Ok(false)` if it did not exist (Row 24).
pub fn delete_branch(repo_path: &Path, branch: &str) -> ShakoResult<bool> {
    validate_branch_name(branch)?;
    let repo = open_isolated(repo_path)?;
    let full_name: FullName = format!("refs/heads/{branch}")
        .try_into()
        .map_err(|e| ShakoError::Unprocessable(format!("invalid branch name: {e}")))?;
    if repo
        .refs
        .try_find(full_name.as_bstr())
        .map_err(|e| ShakoError::Git(e.to_string()))?
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
        .map_err(|e| ShakoError::Git(format!("delete branch prepare: {e}")))?
        .commit(None)
        .map_err(|e| ShakoError::Git(format!("delete branch commit: {e}")))?;
    Ok(true)
}

/// Sets a reference (such as `refs/pull/{number}/head`) directly to `oid_hex`.
pub fn set_ref_direct(repo_path: &Path, full_ref_str: &str, oid_hex: &str) -> ShakoResult<()> {
    validate_full_ref_name(full_ref_str)?;
    let repo = open_isolated(repo_path)?;
    let oid = ObjectId::from_hex(oid_hex.as_bytes())
        .map_err(|e| ShakoError::BadRequest(format!("invalid object id '{oid_hex}': {e}")))?;
    let full_name: FullName = full_ref_str
        .try_into()
        .map_err(|e| ShakoError::Unprocessable(format!("invalid ref: {e}")))?;
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
        .map_err(|e| ShakoError::Git(e.to_string()))?
        .commit(None)
        .map_err(|e| ShakoError::Git(e.to_string()))?;
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
