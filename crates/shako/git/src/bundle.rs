#![forbid(unsafe_code)]
//! Git bundle v2 writer (`# v2 git bundle\n`) for single-ref export (`archive/{ref}.bundle`,
//! Row 32, Contract S11) and full-repository backup bundles (Contract S6).

use crate::pack::{Admit, write_pack};
use crate::repo::{open_isolated, resolve_commit_oid};
use crate::upload_pack::collect_advertised_refs;
use crate::{git_err, storage_err};
use gix_hash::ObjectId;
use shako_core::{ShakoError, ShakoResult};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

/// Writes a Git v2 bundle for a single ref `git_ref`, advertising `refs/heads/bundle` and `HEAD`
/// as `portal:src/git/gitea.rs:2257` (Row 32, Contract S11) expects.
pub fn create_single_ref_bundle(
    repo_path: &Path,
    git_ref: &str,
    mut out: impl Write,
    admit: Admit<'_>,
) -> ShakoResult<()> {
    let repo = open_isolated(repo_path)?;
    let tip_oid = resolve_commit_oid(&repo, git_ref)?;
    let objects = collect_all_objects_from_tips(&repo, &[tip_oid])?;
    out.write_all(
        format!("# v2 git bundle\n{tip_oid} refs/heads/bundle\n{tip_oid} HEAD\n\n").as_bytes(),
    )
    .map_err(storage_err("bundle write"))?;
    write_pack(&repo, &objects, out, admit)
}

/// Writes a full backup Git v2 bundle containing every branch and tag of the repository (Contract S6).
pub fn create_full_backup_bundle(repo_path: &Path) -> ShakoResult<Vec<u8>> {
    let repo = open_isolated(repo_path)?;
    let (_, refs) = collect_advertised_refs(&repo, false)?;
    let mut tips = Vec::new();
    let mut header_lines = String::from("# v2 git bundle\n");

    for r in &refs {
        header_lines.push_str(&format!("{} {}\n", r.oid, r.name));
        if let Some(peeled) = r.peeled {
            tips.push(peeled);
        } else if repo.find_commit(r.oid).is_ok() {
            tips.push(r.oid);
        }
    }
    header_lines.push('\n');

    let mut objects = collect_all_objects_from_tips(&repo, &tips)?;
    for r in &refs {
        if !objects.contains(&r.oid) && repo.has_object(r.oid) {
            objects.push(r.oid);
        }
    }
    let mut out = header_lines.into_bytes();
    write_pack(&repo, &objects, &mut out, &mut |_| Ok(()))?;
    Ok(out)
}

/// Restores a bare repository from a Git v2 bundle created by [`create_full_backup_bundle`] (Contract S6).
pub fn restore_from_bundle(repo_path: &Path, bundle_bytes: &[u8]) -> ShakoResult<()> {
    let Some(header_end) = bundle_bytes.windows(2).position(|w| w == b"\n\n") else {
        return Err(ShakoError::BadRequest(
            "bundle missing header terminator".into(),
        ));
    };
    let header_str = std::str::from_utf8(&bundle_bytes[..header_end])
        .map_err(|_| ShakoError::BadRequest("bundle header is not UTF-8".into()))?;
    let pack = &bundle_bytes[header_end + 2..];
    if !pack.starts_with(b"PACK") {
        return Err(ShakoError::BadRequest("bundle contains no PACK".into()));
    }

    let pack_dir = repo_path.join("objects/pack");
    std::fs::create_dir_all(&pack_dir).map_err(storage_err("mkdir pack dir"))?;
    let mut cursor = std::io::Cursor::new(pack);
    let mut progress = gix::progress::Discard;
    let should_interrupt = std::sync::atomic::AtomicBool::new(false);
    let outcome = gix_pack::Bundle::write_to_directory(
        &mut cursor,
        Some(&pack_dir),
        &mut progress,
        &should_interrupt,
        None::<gix_odb::Handle>,
        gix_hash::Kind::Sha1,
        gix_pack::bundle::write::Options::default(),
    )
    .map_err(git_err("restore bundle index-pack failed"))?;
    if let Some(keep) = outcome.keep_path {
        let _ = std::fs::remove_file(keep);
    }

    for line in header_str.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('-') || line.starts_with('@') {
            continue;
        }
        if let Some((oid_s, refname)) = line.split_once(' ') {
            if refname == "HEAD" {
                continue;
            }
            crate::repo::set_ref_direct(repo_path, refname.trim(), oid_s.trim())?;
        }
    }
    Ok(())
}

pub(crate) fn collect_all_objects_from_tips(
    repo: &gix::Repository,
    tips: &[ObjectId],
) -> ShakoResult<Vec<ObjectId>> {
    let mut set = BTreeSet::new();
    if tips.is_empty() {
        return Ok(Vec::new());
    }
    let walk = repo
        .rev_walk(tips.iter().copied())
        .all()
        .map_err(git_err("bundle rev_walk failed"))?;
    // Strict: compaction deletes what this misses, so an object it cannot read stops it.
    for item in walk {
        let item = item.map_err(git_err("object walk failed"))?;
        set.insert(item.id);
        let tree = item
            .object()
            .map_err(git_err("object walk failed"))?
            .tree()
            .map_err(git_err("object walk failed"))?;
        collect_all_tree_objects(repo, &tree, &mut set)?;
    }
    Ok(set.into_iter().collect())
}

pub(crate) fn collect_all_tree_objects(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    out: &mut BTreeSet<ObjectId>,
) -> ShakoResult<()> {
    if !out.insert(tree.id) {
        return Ok(());
    }
    let decoded = tree.decode().map_err(git_err("decode tree"))?;
    for entry in decoded.entries {
        if entry.mode.is_tree() {
            let sub = repo.find_tree(entry.oid).map_err(git_err("find subtree"))?;
            collect_all_tree_objects(repo, &sub, out)?;
        } else if !entry.mode.is_commit() {
            out.insert(entry.oid.to_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{create_branch, list_branches, list_tags, set_ref_direct};
    use crate::testutil::{annotated_tag, put, repo};

    #[test]
    fn a_full_backup_restores_every_branch_and_tag_into_an_empty_repository() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let source = repo(tmp.path(), 1, true);
        let tip = put(&source, "main", "a.txt", b"a\n");
        create_branch(&source, "feature", "main").expect("branch");
        put(&source, "feature", "f.txt", b"f\n");
        annotated_tag(&source, "v1", tip, gix_object::Kind::Commit);
        let blob = gix_object::compute_hash(gix_hash::Kind::Sha1, gix_object::Kind::Blob, b"a\n")
            .expect("hash");
        set_ref_direct(&source, "refs/tags/blob", &blob.to_string()).expect("blob tag");

        let backup = create_full_backup_bundle(&source).expect("backup");
        let header = String::from_utf8_lossy(
            &backup[..backup.windows(2).position(|w| w == b"\n\n").expect("end")],
        )
        .into_owned();
        assert!(header.starts_with("# v2 git bundle\n"), "{header}");
        assert!(
            header.contains(&format!("{tip} HEAD")) && header.contains("refs/tags/blob"),
            "{header}"
        );

        let dest = repo(tmp.path(), 2, false);
        restore_from_bundle(&dest, &backup).expect("restore");
        assert_eq!(
            list_branches(&dest).expect("b"),
            list_branches(&source).expect("b")
        );
        assert_eq!(list_tags(&dest).expect("t"), list_tags(&source).expect("t"));
        assert!(open_isolated(&dest).expect("open").has_object(blob));
        // An empty repository backs up to a header alone and an empty pack.
        let empty = create_full_backup_bundle(&repo(tmp.path(), 3, false)).expect("empty");
        assert!(empty.starts_with(b"# v2 git bundle\n\nPACK"));
    }

    #[test]
    fn a_restore_refuses_a_bundle_without_header_end_utf8_header_or_pack() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = repo(tmp.path(), 1, false);
        for (bytes, says) in [
            (&b"# v2 git bundle\n"[..], "missing header terminator"),
            (&b"\xff\xfe\n\nPACK"[..], "not UTF-8"),
            (&b"# v2 git bundle\n\nJUNK"[..], "contains no PACK"),
        ] {
            let err = restore_from_bundle(&dest, bytes).expect_err(says);
            assert_eq!(err.status_code(), 400, "{err}");
            assert!(err.to_string().contains(says), "{err}");
        }
        let err =
            restore_from_bundle(&dest, b"# v2 git bundle\n\nPACKgarbage").expect_err("corrupt");
        assert!(
            matches!(&err, ShakoError::Git(m) if m.contains("index-pack failed")),
            "{err}"
        );
    }

    #[test]
    fn a_restore_skips_prerequisites_options_and_head_and_refuses_a_bad_ref_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let source = repo(tmp.path(), 1, true);
        let tip = put(&source, "main", "a.txt", b"a\n");
        let mut single = Vec::new();
        create_single_ref_bundle(
            &source,
            "main",
            &mut single,
            &mut crate::testutil::admit_all,
        )
        .expect("bundle");
        let pack = &single[single.windows(2).position(|w| w == b"\n\n").expect("end") + 2..];
        let with = |header: &str| [header.as_bytes(), b"\n\n", pack].concat();
        let dest = repo(tmp.path(), 2, false);
        let header = format!(
            "# v2 git bundle\n-{tip} a prerequisite\n@filter blob:none\n\n  \nno-space-here\n{tip} HEAD\n{tip} refs/heads/restored"
        );
        // The blank line inside the header ends it early: the lines after it are pack garbage.
        let err =
            restore_from_bundle(&dest, &with(&header)).expect_err("header ends at the blank line");
        assert_eq!(err.status_code(), 400);
        let header = format!(
            "# v2 git bundle\n-{tip} a prerequisite\n@filter blob:none\n  \nno-space-here\n{tip} HEAD\n{tip} refs/heads/restored"
        );
        restore_from_bundle(&dest, &with(&header)).expect("restore");
        assert_eq!(
            list_branches(&dest).expect("b"),
            [("restored".to_owned(), tip.to_string())]
        );
        let bad = format!("# v2 git bundle\n{tip} refs/heads/a..b");
        assert!(restore_from_bundle(&dest, &with(&bad)).is_err());
    }

    #[test]
    fn a_single_ref_bundle_names_its_tip_and_needs_a_known_ref_and_admission() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let source = repo(tmp.path(), 1, true);
        let tip = put(&source, "main", "a.txt", b"a\n");
        let mut out = Vec::new();
        create_single_ref_bundle(
            &source,
            &tip.to_string(),
            &mut out,
            &mut crate::testutil::admit_all,
        )
        .expect("bundle");
        assert!(out.starts_with(
            format!("# v2 git bundle\n{tip} refs/heads/bundle\n{tip} HEAD\n\nPACK").as_bytes()
        ));
        let err =
            create_single_ref_bundle(&source, "nope", Vec::new(), &mut crate::testutil::admit_all)
                .expect_err("unknown");
        assert_eq!(err.status_code(), 404);
        let err = create_single_ref_bundle(&source, "main", Vec::new(), &mut |_| {
            Err(ShakoError::Overloaded("busy".into()))
        })
        .expect_err("refused");
        assert!(matches!(err, ShakoError::Overloaded(_)));
        let err = create_single_ref_bundle(
            &source,
            "main",
            crate::testutil::Refusing,
            &mut crate::testutil::admit_all,
        )
        .expect_err("writer");
        assert!(matches!(err, ShakoError::Storage(_)), "{err}");
        let handle = open_isolated(&source).expect("open");
        assert!(
            collect_all_objects_from_tips(&handle, &[])
                .expect("none")
                .is_empty()
        );
    }
}
