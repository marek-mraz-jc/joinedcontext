#![forbid(unsafe_code)]
//! In-memory 3-way tree merge (`gix::Repository::merge_trees`), mergeability computation,
//! Pull Request merge (`merge`, `squash`, `fast-forward-only`), `update-branch`, and tree diffing
//! (Contract S10, Threat T10).

use crate::git_err;
use crate::repo::{
    CommitAuthor, build_nested_trees, collect_tree_blobs, is_ancestor, open_isolated,
    resolve_commit_oid, sanitize_actor_field, write_objects_as_pack,
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

/// Merged tree as flat `path -> (kind, blob id)`: a clean merge takes each entry from one side.
type MergedTree = BTreeMap<String, (EntryKind, ObjectId)>;

/// Performs a 3-way tree merge in memory between `ancestor_oid`, `our_oid`, and `their_oid`,
/// path by path: a file both sides changed differently is a conflict (no line-level merge).
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
        .map_err(git_err("find our commit"))?;
    let our_tree = our_commit.tree().map_err(git_err("find our tree"))?;
    let mut our_files = BTreeMap::new();
    collect_tree_blobs(repo, &our_tree, "", &mut our_files)?;

    let their_commit = repo
        .find_commit(their_oid)
        .map_err(git_err("find their commit"))?;
    let their_tree = their_commit.tree().map_err(git_err("find their tree"))?;
    let mut their_files = BTreeMap::new();
    collect_tree_blobs(repo, &their_tree, "", &mut their_files)?;

    let mut all_paths = std::collections::BTreeSet::new();
    all_paths.extend(base_files.keys().cloned());
    all_paths.extend(our_files.keys().cloned());
    all_paths.extend(their_files.keys().cloned());

    let mut merged = MergedTree::new();
    let mut conflicts = Vec::new();

    for path in all_paths {
        let b = base_files.get(&path).copied();
        let o = our_files.get(&path).copied();
        let t = their_files.get(&path).copied();

        match (b, o, t) {
            (_, Some(ours), Some(theirs)) if ours == theirs => {
                merged.insert(path, ours);
            }
            (Some(base), Some(ours), Some(theirs)) if base == ours => {
                merged.insert(path, theirs);
            }
            (Some(base), Some(ours), Some(theirs)) if base == theirs => {
                merged.insert(path, ours);
            }
            (Some(base), Some(ours), None) if base == ours => {
                // Deleted cleanly by theirs
            }
            (Some(base), None, Some(theirs)) if base == theirs => {
                // Deleted cleanly by ours
            }
            (None, Some(ours), None) => {
                merged.insert(path, ours);
            }
            (None, None, Some(theirs)) => {
                merged.insert(path, theirs);
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
    let root_tree_oid = build_nested_trees(&merged_map, &mut new_objects)?;

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
        .map_err(git_err("serialize merge commit"))?;
    let commit_oid =
        gix_object::compute_hash(gix_hash::Kind::Sha1, gix_object::Kind::Commit, &commit_raw)
            .map_err(git_err("hash merge commit"))?;
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
        .map_err(git_err("find base commit"))?;
    let base_tree = base_commit.tree().map_err(git_err("find base tree"))?;
    let mut base_files = BTreeMap::new();
    collect_tree_blobs(&repo, &base_tree, "", &mut base_files)?;

    let head_commit = repo
        .find_commit(head_oid)
        .map_err(git_err("find head commit"))?;
    let head_tree = head_commit.tree().map_err(git_err("find head tree"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{create_branch, list_tree_entries, read_commit_info, read_file_at_ref};
    use crate::testutil::{author, put, remove, repo};

    fn opts<'a>(style: MergeStyle, expected_head_sha: Option<&'a str>) -> MergeOptions<'a> {
        static AUTHOR: std::sync::LazyLock<CommitAuthor> = std::sync::LazyLock::new(author);
        MergeOptions {
            expected_head_sha,
            style,
            message: "merge",
            author: &AUTHOR,
            committer: &AUTHOR,
        }
    }

    fn file(path: &Path, branch: &str, name: &str) -> Option<String> {
        read_file_at_ref(path, branch, name, &mut crate::testutil::admit_all)
            .expect("read")
            .map(|(_, bytes)| String::from_utf8_lossy(&bytes).into_owned())
    }

    #[test]
    fn a_merge_style_parses_by_name_and_refuses_rebase_and_unknown_ones() {
        assert_eq!(
            MergeStyle::parse("merge").expect("merge"),
            MergeStyle::Merge
        );
        assert_eq!(
            MergeStyle::parse("SQUASH").expect("squash"),
            MergeStyle::Squash
        );
        for ff in ["fast-forward-only", "fast_forward_only"] {
            assert_eq!(
                MergeStyle::parse(ff).expect(ff),
                MergeStyle::FastForwardOnly
            );
        }
        for (style, says) in [
            ("rebase", "not supported"),
            ("rebase-merge", "not supported"),
            ("octopus", "unknown"),
        ] {
            let err = MergeStyle::parse(style).expect_err(style);
            assert_eq!(err.status_code(), 422);
            assert!(err.to_string().contains(says), "{err}");
        }
    }

    #[test]
    fn a_three_way_merge_takes_each_side_change_and_names_every_conflict() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, false);
        for name in [
            "same",
            "ours",
            "theirs",
            "both",
            "del-theirs",
            "del-ours",
            "del-both",
            "clash",
            "mod-del",
        ] {
            put(&path, "main", &format!("{name}.txt"), b"base\n");
        }
        create_branch(&path, "head", "main").expect("branch");
        // main is ours, head is theirs.
        for (name, body) in [
            ("ours", "o"),
            ("both", "same"),
            ("clash", "o"),
            ("mod-del", "o"),
            ("add-ours", "o"),
            ("add-both", "o"),
        ] {
            put(&path, "main", &format!("{name}.txt"), body.as_bytes());
        }
        for name in ["del-ours", "del-both"] {
            remove(&path, "main", &format!("{name}.txt"));
        }
        for (name, body) in [
            ("theirs", "t"),
            ("both", "same"),
            ("clash", "t"),
            ("add-theirs", "t"),
            ("add-both", "t"),
        ] {
            put(&path, "head", &format!("{name}.txt"), body.as_bytes());
        }
        for name in ["del-theirs", "del-both", "mod-del"] {
            remove(&path, "head", &format!("{name}.txt"));
        }
        let check = check_mergeability(&path, "main", "head").expect("check");
        assert!(!check.mergeable && check.is_behind_base);
        assert_eq!(
            check.conflicting_paths,
            ["add-both.txt", "clash.txt", "mod-del.txt"]
        );
        assert_eq!(check.merge_base.len(), 40);
        let err = execute_pr_merge(&path, "main", "head", &opts(MergeStyle::Merge, None))
            .expect_err("conflicts");
        assert_eq!(
            err.to_string(),
            "merge conflict in: add-both.txt, clash.txt, mod-del.txt"
        );
        assert_eq!(err.status_code(), 409);

        // head takes main's side of each conflict; the merge then holds every clean change.
        for name in ["clash", "mod-del", "add-both"] {
            put(&path, "head", &format!("{name}.txt"), b"o");
        }
        let head = check_mergeability(&path, "main", "head")
            .expect("check")
            .head_sha;
        let merged = execute_pr_merge(&path, "main", "head", &opts(MergeStyle::Merge, Some(&head)))
            .expect("merge");
        let info = read_commit_info(&path, &merged).expect("info");
        assert_eq!(info.parent_shas.len(), 2);
        assert_eq!(info.parent_shas[1], head);
        let (_, entries) = list_tree_entries(&path, "main", false).expect("tree");
        let names: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            names,
            [
                "add-both.txt",
                "add-ours.txt",
                "add-theirs.txt",
                "both.txt",
                "clash.txt",
                "mod-del.txt",
                "ours.txt",
                "same.txt",
                "theirs.txt"
            ]
        );
        for (name, body) in [
            ("ours", "o"),
            ("theirs", "t"),
            ("same", "base\n"),
            ("both", "same"),
            ("add-theirs", "t"),
        ] {
            assert_eq!(
                file(&path, "main", &format!("{name}.txt")).as_deref(),
                Some(body),
                "{name}"
            );
        }
    }

    #[test]
    fn squash_fast_forward_and_a_moved_head_each_behave_as_named() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        create_branch(&path, "same", "main").expect("branch");
        create_branch(&path, "feature", "main").expect("branch");
        let tip = put(&path, "feature", "f.txt", b"f").to_string();

        let err = execute_pr_merge(
            &path,
            "main",
            "feature",
            &opts(MergeStyle::Squash, Some("0000")),
        )
        .expect_err("moved");
        assert_eq!(err.status_code(), 409);
        let ff = execute_pr_merge(
            &path,
            "main",
            "feature",
            &opts(MergeStyle::FastForwardOnly, Some("")),
        )
        .expect("ff");
        assert_eq!(ff, tip);
        // `same` takes a commit of its own: main is no ancestor of it any more.
        put(&path, "same", "s.txt", b"s");
        let err = execute_pr_merge(
            &path,
            "main",
            "same",
            &opts(MergeStyle::FastForwardOnly, None),
        )
        .expect_err("diverged");
        assert_eq!(err.status_code(), 409);
        assert!(err.to_string().contains("cannot fast-forward"), "{err}");

        let squashed = execute_pr_merge(&path, "main", "same", &opts(MergeStyle::Squash, None))
            .expect("squash");
        let info = read_commit_info(&path, &squashed).expect("info");
        assert_eq!(
            (info.parent_shas, info.message),
            (vec![tip], "merge".to_owned())
        );
        assert_eq!(file(&path, "main", "s.txt").as_deref(), Some("s"));
        // A merge of a branch into itself has one parent.
        let again =
            execute_pr_merge(&path, "main", "main", &opts(MergeStyle::Merge, None)).expect("self");
        assert_eq!(
            read_commit_info(&path, &again).expect("info").parent_shas,
            [squashed]
        );
        for (base, head) in [("nope", "main"), ("main", "nope")] {
            let err = execute_pr_merge(&path, base, head, &opts(MergeStyle::Merge, None))
                .expect_err("unknown");
            assert_eq!(err.status_code(), 404);
            assert_eq!(
                check_mergeability(&path, base, head)
                    .expect_err("unknown")
                    .status_code(),
                404
            );
            assert_eq!(
                diff_refs_files(&path, base, head)
                    .expect_err("unknown")
                    .status_code(),
                404
            );
            assert_eq!(
                update_pr_branch(&path, base, head, &author())
                    .expect_err("unknown")
                    .status_code(),
                404
            );
        }
    }

    #[test]
    fn updating_a_branch_merges_its_base_in_only_when_it_is_behind() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        create_branch(&path, "feature", "main").expect("branch");
        let ahead = put(&path, "feature", "f.txt", b"f").to_string();
        assert_eq!(
            update_pr_branch(&path, "main", "feature", &author()).expect("current"),
            ahead
        );
        let base = put(&path, "main", "m.txt", b"m").to_string();
        let updated = update_pr_branch(&path, "main", "feature", &author()).expect("update");
        let info = read_commit_info(&path, &updated).expect("info");
        assert_eq!(info.message, "Merge branch 'main' into feature");
        assert_eq!(info.parent_shas, [ahead, base]);
        assert_eq!(file(&path, "feature", "m.txt").as_deref(), Some("m"));
        let check = check_mergeability(&path, "main", "feature").expect("check");
        assert!(check.mergeable && !check.is_behind_base);
    }

    #[test]
    fn a_diff_names_files_added_modified_deleted_and_renamed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, false);
        for (name, body) in [
            ("keep", "k"),
            ("mod", "m"),
            ("del", "d"),
            ("old-name", "moved"),
        ] {
            put(&path, "main", &format!("{name}.txt"), body.as_bytes());
        }
        create_branch(&path, "head", "main").expect("branch");
        put(&path, "head", "mod.txt", b"changed");
        remove(&path, "head", "del.txt");
        remove(&path, "head", "old-name.txt");
        put(&path, "head", "new-name.txt", b"moved");
        put(&path, "head", "added.txt", b"a");
        let diff = diff_refs_files(&path, "main", "head").expect("diff");
        let got: Vec<_> = diff
            .iter()
            .map(|d| (d.filename.as_str(), d.status.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("added.txt", "added"),
                ("del.txt", "deleted"),
                ("mod.txt", "modified"),
                ("new-name.txt", "renamed")
            ]
        );
        // Unrelated histories diff against the base itself.
        // Other content than main's first commit, which would otherwise hash the same.
        put(&path, "orphan", "keep.txt", b"elsewhere");
        let diff = diff_refs_files(&path, "main", "orphan").expect("diff");
        let got: Vec<_> = diff
            .iter()
            .map(|d| (d.filename.as_str(), d.status.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("del.txt", "deleted"),
                ("keep.txt", "modified"),
                ("mod.txt", "deleted"),
                ("old-name.txt", "deleted")
            ]
        );
        let check = check_mergeability(&path, "main", "orphan").expect("check");
        assert_eq!(check.merge_base, "");
        assert!(check.is_behind_base);
    }
}
