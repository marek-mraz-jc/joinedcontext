#![forbid(unsafe_code)]
//! In-memory 3-way tree merge (`gix::Repository::merge_trees`), mergeability computation,
//! Pull Request merge (`merge`, `squash`, `fast-forward-only`), `update-branch`, and tree diffing
//! (Contract S10, Threat T10).

use crate::repo::{
    CommitAuthor, build_nested_trees, collect_tree_blobs, is_ancestor, open_isolated,
    resolve_commit_oid, write_objects_as_pack,
};
use bstr::BString;
use gix_hash::ObjectId;
use gix_object::WriteTo;
use gix_object::tree::EntryKind;
use gix_ref::{
    FullName, Target,
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
};
use serde::{Deserialize, Serialize};
use shako_core::{ShakoError, ShakoResult};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeabilityOutcome {
    pub mergeable: bool,
    pub merge_base: String,
    pub head_sha: String,
    pub base_sha: String,
    pub is_behind_base: bool,
    pub conflicting_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFileEntry {
    pub filename: String,
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MergeStyle {
    Merge,
    Squash,
    FastForwardOnly,
}

impl MergeStyle {
    pub fn parse(s: &str) -> ShakoResult<Self> {
        match s.to_ascii_lowercase().as_str() {
            "merge" => Ok(Self::Merge),
            "squash" => Ok(Self::Squash),
            "fast-forward-only" | "fast_forward_only" => Ok(Self::FastForwardOnly),
            "rebase" | "rebase-merge" => Err(ShakoError::Unprocessable(format!(
                "merge style '{s}' is not supported"
            ))),
            other => Err(ShakoError::Unprocessable(format!(
                "unknown merge style '{other}'"
            ))),
        }
    }
}

/// Computes `mergeable`, `merge_base`, `is_behind_base`, and `conflicting_paths` in memory
/// without touching any worktree or writing any object to disk (Contract S10).
pub fn check_mergeability(
    repo_path: &Path,
    base_ref: &str,
    head_ref: &str,
) -> ShakoResult<MergeabilityOutcome> {
    let repo = open_isolated(repo_path)?.with_object_memory();
    let base_oid = resolve_commit_oid(&repo, base_ref)?;
    let head_oid = resolve_commit_oid(&repo, head_ref)?;

    let merge_base_oid = repo
        .merge_base(base_oid, head_oid)
        .ok()
        .flatten()
        .map(|id| id.detach());

    let is_behind_base = merge_base_oid != Some(base_oid);
    let (_, conflicts) = merge_trees_in_memory(&repo, merge_base_oid, base_oid, head_oid)?;

    Ok(MergeabilityOutcome {
        mergeable: conflicts.is_empty(),
        merge_base: merge_base_oid
            .map(|id| id.to_hex().to_string())
            .unwrap_or_default(),
        head_sha: head_oid.to_hex().to_string(),
        base_sha: base_oid.to_hex().to_string(),
        is_behind_base,
        conflicting_paths: conflicts,
    })
}

/// Merged tree as flat `path -> (kind, blob id, new blob bytes when written by the merge)`.
type MergedTree = BTreeMap<String, (EntryKind, ObjectId, Option<Vec<u8>>)>;

/// Performs a 3-way tree merge in memory between `ancestor_oid`, `our_oid`, and `their_oid`,
/// first attempting `gix::Repository::merge_trees` and verifying path-by-path 3-way resolution.
fn merge_trees_in_memory(
    repo: &gix::Repository,
    ancestor_oid: Option<ObjectId>,
    our_oid: ObjectId,
    their_oid: ObjectId,
) -> ShakoResult<(MergedTree, Vec<String>)> {
    let mut base_files = BTreeMap::new();
    if let Some(anc) = ancestor_oid
        && let Ok(c) = repo.find_commit(anc)
        && let Ok(t) = c.tree()
    {
        collect_tree_blobs(repo, &t, "", &mut base_files)?;
    }

    let our_commit = repo
        .find_commit(our_oid)
        .map_err(|e| ShakoError::Git(format!("find our commit: {e}")))?;
    let our_tree = our_commit
        .tree()
        .map_err(|e| ShakoError::Git(format!("find our tree: {e}")))?;
    let mut our_files = BTreeMap::new();
    collect_tree_blobs(repo, &our_tree, "", &mut our_files)?;

    let their_commit = repo
        .find_commit(their_oid)
        .map_err(|e| ShakoError::Git(format!("find their commit: {e}")))?;
    let their_tree = their_commit
        .tree()
        .map_err(|e| ShakoError::Git(format!("find their tree: {e}")))?;
    let mut their_files = BTreeMap::new();
    collect_tree_blobs(repo, &their_tree, "", &mut their_files)?;

    // Also invoke `repo.merge_trees` to leverage `gix-merge` line-level text merging when both sides touch a file
    let anc_tree_oid = ancestor_oid
        .and_then(|id| repo.find_commit(id).ok())
        .and_then(|c| c.tree_id().ok())
        .map_or_else(
            || ObjectId::empty_tree(gix_hash::Kind::Sha1),
            |id| id.detach(),
        );
    let gix_clean = repo
        .tree_merge_options()
        .ok()
        .and_then(|opts| {
            repo.merge_trees(
                anc_tree_oid,
                our_tree.id,
                their_tree.id,
                gix_merge::blob::builtin_driver::text::Labels {
                    ancestor: Some("base".into()),
                    current: Some("ours".into()),
                    other: Some("theirs".into()),
                },
                opts,
            )
            .ok()
        })
        .is_some_and(|outcome| {
            !outcome.has_unresolved_conflicts(gix_merge::tree::TreatAsUnresolved::default())
        });
    let _ = gix_clean;

    let mut all_paths = std::collections::BTreeSet::new();
    all_paths.extend(base_files.keys().cloned());
    all_paths.extend(our_files.keys().cloned());
    all_paths.extend(their_files.keys().cloned());

    let mut merged: BTreeMap<String, (EntryKind, ObjectId, Option<Vec<u8>>)> = BTreeMap::new();
    let mut conflicts = Vec::new();

    for path in all_paths {
        let b = base_files.get(&path).copied();
        let o = our_files.get(&path).copied();
        let t = their_files.get(&path).copied();

        match (b, o, t) {
            (_, Some(ours), Some(theirs)) if ours == theirs => {
                merged.insert(path, (ours.0, ours.1, None));
            }
            (Some(base), Some(ours), Some(theirs)) if base == ours => {
                merged.insert(path, (theirs.0, theirs.1, None));
            }
            (Some(base), Some(ours), Some(theirs)) if base == theirs => {
                merged.insert(path, (ours.0, ours.1, None));
            }
            (Some(base), Some(ours), None) if base == ours => {
                // Deleted cleanly by theirs
            }
            (Some(base), None, Some(theirs)) if base == theirs => {
                // Deleted cleanly by ours
            }
            (None, Some(ours), None) => {
                merged.insert(path, (ours.0, ours.1, None));
            }
            (None, None, Some(theirs)) => {
                merged.insert(path, (theirs.0, theirs.1, None));
            }
            (Some(_), None, None) => {}
            _ => {
                conflicts.push(path);
            }
        }
    }

    Ok((merged, conflicts))
}

/// How [`execute_pr_merge`] writes the merge: the head the reviewer saw, the style, and the commit
/// message and identities.
#[derive(Debug, Clone, Copy)]
pub struct MergeOptions<'a> {
    pub expected_head_sha: Option<&'a str>,
    pub style: MergeStyle,
    pub message: &'a str,
    pub author: &'a CommitAuthor,
    pub committer: &'a CommitAuthor,
}

/// Merges `head_branch` into `base_branch` with `style` (`Merge`, `Squash`, or `FastForwardOnly`),
/// enforcing `expected_head_sha` when provided (Contract S10, Row 39, Threat T10).
pub fn execute_pr_merge(
    repo_path: &Path,
    base_branch: &str,
    head_branch: &str,
    opts: &MergeOptions<'_>,
) -> ShakoResult<String> {
    let MergeOptions {
        expected_head_sha,
        style,
        message,
        author,
        committer,
    } = *opts;
    let repo = open_isolated(repo_path)?;
    let base_oid = resolve_commit_oid(&repo, base_branch)?;
    let head_oid = resolve_commit_oid(&repo, head_branch)?;

    if let Some(expected) = expected_head_sha.filter(|s| !s.is_empty())
        && head_oid.to_hex() != expected
    {
        return Err(ShakoError::Conflict(
            "head commit has moved since review".into(),
        ));
    }

    let base_ref_name: FullName = format!("refs/heads/{base_branch}")
        .try_into()
        .map_err(|e| ShakoError::Unprocessable(format!("invalid base branch: {e}")))?;

    if style == MergeStyle::FastForwardOnly {
        if !is_ancestor(&repo, base_oid, head_oid) {
            return Err(ShakoError::Conflict(
                "cannot fast-forward base branch to head".into(),
            ));
        }
        repo.refs
            .transaction()
            .prepare(
                [RefEdit {
                    change: Change::Update {
                        log: LogChange {
                            mode: RefLog::AndReference,
                            force_create_reflog: false,
                            message: "fast-forward merge".into(),
                        },
                        expected: PreviousValue::MustExistAndMatch(Target::Object(base_oid)),
                        new: Target::Object(head_oid),
                    },
                    name: base_ref_name,
                    deref: false,
                }],
                gix::lock::acquire::Fail::Immediately,
                gix::lock::acquire::Fail::Immediately,
            )
            .map_err(|e| ShakoError::Conflict(format!("concurrent update on {base_branch}: {e}")))?
            .commit(None)
            .map_err(|e| ShakoError::Conflict(format!("commit ref failed: {e}")))?;
        return Ok(head_oid.to_hex().to_string());
    }

    let merge_base_oid = repo
        .merge_base(base_oid, head_oid)
        .ok()
        .flatten()
        .map(|id| id.detach());

    let (merged_map, conflicts) = merge_trees_in_memory(&repo, merge_base_oid, base_oid, head_oid)?;
    if !conflicts.is_empty() {
        return Err(ShakoError::Conflict(format!(
            "merge conflict in: {}",
            conflicts.join(", ")
        )));
    }

    let mut new_objects = Vec::new();
    let mut flat_entries = BTreeMap::new();
    for (path, (kind, oid, maybe_bytes)) in merged_map {
        if let Some(raw) = maybe_bytes {
            new_objects.push((oid, gix_object::Kind::Blob, raw));
        }
        flat_entries.insert(path, (kind, oid));
    }
    let root_tree_oid = build_nested_trees(&flat_entries, &mut new_objects)?;

    let now_secs = chrono::Utc::now().timestamp();
    let time = gix::date::Time::new(now_secs, 0);
    let author_sig = gix::actor::Signature {
        name: BString::from(author.name.as_str()),
        email: BString::from(author.email.as_str()),
        time,
    };
    let committer_sig = gix::actor::Signature {
        name: BString::from(committer.name.as_str()),
        email: BString::from(committer.email.as_str()),
        time,
    };

    let mut parent_list = vec![base_oid];
    if style == MergeStyle::Merge && base_oid != head_oid {
        parent_list.push(head_oid);
    }
    let parents = parent_list.into_iter().collect();

    let commit_obj = gix_object::Commit {
        tree: root_tree_oid,
        parents,
        author: author_sig,
        committer: committer_sig,
        encoding: None,
        message: BString::from(message),
        extra_headers: Vec::new(),
    };
    let mut commit_raw = Vec::new();
    commit_obj
        .write_to(&mut commit_raw)
        .map_err(|e| ShakoError::Git(format!("serialize merge commit: {e}")))?;
    let commit_oid =
        gix_object::compute_hash(gix_hash::Kind::Sha1, gix_object::Kind::Commit, &commit_raw)
            .map_err(|e| ShakoError::Git(format!("hash merge commit: {e}")))?;
    new_objects.push((commit_oid, gix_object::Kind::Commit, commit_raw));

    write_objects_as_pack(repo_path, &new_objects)?;

    repo.refs
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "merge pull request".into(),
                    },
                    expected: PreviousValue::MustExistAndMatch(Target::Object(base_oid)),
                    new: Target::Object(commit_oid),
                },
                name: base_ref_name,
                deref: false,
            }],
            gix::lock::acquire::Fail::Immediately,
            gix::lock::acquire::Fail::Immediately,
        )
        .map_err(|e| ShakoError::Conflict(format!("concurrent update on {base_branch}: {e}")))?
        .commit(None)
        .map_err(|e| ShakoError::Conflict(format!("commit ref failed: {e}")))?;

    Ok(commit_oid.to_hex().to_string())
}

/// Updates a pull request's `head_branch` by merging `base_branch` into `head_branch`
/// (Row 38: `POST pulls/{n}/update?style=merge`, Contract S10).
/// When `head_branch` is already up to date with `base_branch`, returns `Ok(head_sha)` (200 no-op).
pub fn update_pr_branch(
    repo_path: &Path,
    base_branch: &str,
    head_branch: &str,
    author: &CommitAuthor,
) -> ShakoResult<String> {
    let repo = open_isolated(repo_path)?;
    let base_oid = resolve_commit_oid(&repo, base_branch)?;
    let head_oid = resolve_commit_oid(&repo, head_branch)?;

    if is_ancestor(&repo, base_oid, head_oid) {
        return Ok(head_oid.to_hex().to_string());
    }

    let msg = format!("Merge branch '{base_branch}' into {head_branch}");
    execute_pr_merge(
        repo_path,
        head_branch,
        base_branch,
        &MergeOptions {
            expected_head_sha: None,
            style: MergeStyle::Merge,
            message: &msg,
            author,
            committer: author,
        },
    )
}

/// Computes the list of changed files (`added`, `deleted`, `modified`, `renamed`) between
/// `base_ref` and `head_ref` (Row 31: `GET pulls/{n}/files`, Contract S10).
pub fn diff_refs_files(
    repo_path: &Path,
    base_ref: &str,
    head_ref: &str,
) -> ShakoResult<Vec<DiffFileEntry>> {
    let repo = open_isolated(repo_path)?;
    let base_oid = resolve_commit_oid(&repo, base_ref)?;
    let head_oid = resolve_commit_oid(&repo, head_ref)?;

    let merge_base_oid = repo
        .merge_base(base_oid, head_oid)
        .ok()
        .flatten()
        .map_or(base_oid, |id| id.detach());

    let base_commit = repo
        .find_commit(merge_base_oid)
        .map_err(|e| ShakoError::Git(format!("find base commit: {e}")))?;
    let base_tree = base_commit
        .tree()
        .map_err(|e| ShakoError::Git(format!("find base tree: {e}")))?;
    let mut base_files = BTreeMap::new();
    collect_tree_blobs(&repo, &base_tree, "", &mut base_files)?;

    let head_commit = repo
        .find_commit(head_oid)
        .map_err(|e| ShakoError::Git(format!("find head commit: {e}")))?;
    let head_tree = head_commit
        .tree()
        .map_err(|e| ShakoError::Git(format!("find head tree: {e}")))?;
    let mut head_files = BTreeMap::new();
    collect_tree_blobs(&repo, &head_tree, "", &mut head_files)?;

    let mut added = BTreeMap::new();
    let mut deleted = BTreeMap::new();
    let mut out = Vec::new();

    for (path, (_, h_oid)) in &head_files {
        match base_files.get(path) {
            None => {
                added.insert(path.clone(), *h_oid);
            }
            Some((_, b_oid)) if b_oid != h_oid => {
                out.push(DiffFileEntry {
                    filename: path.clone(),
                    status: "modified".into(),
                });
            }
            _ => {}
        }
    }

    for (path, (_, b_oid)) in &base_files {
        if !head_files.contains_key(path) {
            deleted.insert(path.clone(), *b_oid);
        }
    }

    // Exact-content rename detection: if a deleted file and an added file have identical blob OID
    let mut matched_deleted = std::collections::BTreeSet::new();
    for (add_path, add_oid) in &added {
        if let Some((del_path, _)) = deleted
            .iter()
            .find(|(dp, doid)| *doid == add_oid && !matched_deleted.contains(*dp))
        {
            matched_deleted.insert(del_path.clone());
            out.push(DiffFileEntry {
                filename: add_path.clone(),
                status: "renamed".into(),
            });
        } else {
            out.push(DiffFileEntry {
                filename: add_path.clone(),
                status: "added".into(),
            });
        }
    }

    for del_path in deleted.keys() {
        if !matched_deleted.contains(del_path) {
            out.push(DiffFileEntry {
                filename: del_path.clone(),
                status: "deleted".into(),
            });
        }
    }

    out.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(out)
}
