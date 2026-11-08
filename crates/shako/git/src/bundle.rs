#![forbid(unsafe_code)]
//! Git bundle v2 writer (`# v2 git bundle\n`) for single-ref export (`archive/{ref}.bundle`,
//! Row 32, Contract S11) and full-repository backup bundles (Contract S6).

use crate::repo::{open_isolated, resolve_commit_oid};
use crate::upload_pack::{ build_pack_bytes, collect_advertised_refs};
use gix_hash::ObjectId;
use shako_core::{ShakoError, ShakoResult};
use std::collections::BTreeSet;
use std::path::Path;

/// Writes a Git v2 bundle for a single ref `git_ref`, advertising `refs/heads/bundle` and `HEAD`
/// as `portal:src/git/gitea.rs:2257` (Row 32, Contract S11) expects.
pub fn create_single_ref_bundle(repo_path: &Path, git_ref: &str) -> ShakoResult<Vec<u8>> {
    let repo = open_isolated(repo_path)?;
    let tip_oid = resolve_commit_oid(&repo, git_ref)?;
    let objects = collect_all_objects_from_tips(&repo, &[tip_oid])?;
    let pack = build_pack_bytes(&repo, &objects, false)?;

    let mut out = Vec::with_capacity(128 + pack.len());
    out.extend_from_slice(b"# v2 git bundle\n");
    out.extend_from_slice(format!("{tip_oid} refs/heads/bundle\n").as_bytes());
    out.extend_from_slice(format!("{tip_oid} HEAD\n\n").as_bytes());
    out.extend_from_slice(&pack);
    Ok(out)
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
    let pack = build_pack_bytes(&repo, &objects, false)?;
    let mut out = header_lines.into_bytes();
    out.extend_from_slice(&pack);
    Ok(out)
}

/// Restores a bare repository from a Git v2 bundle created by [`create_full_backup_bundle`] (Contract S6).
pub fn restore_from_bundle(repo_path: &Path, bundle_bytes: &[u8]) -> ShakoResult<()> {
    let Some(header_end) = bundle_bytes.windows(2).position(|w| w == b"\n\n") else {
        return Err(ShakoError::BadRequest("bundle missing header terminator".into()));
    };
    let header_str = std::str::from_utf8(&bundle_bytes[..header_end])
        .map_err(|_| ShakoError::BadRequest("bundle header is not UTF-8".into()))?;
    let pack = &bundle_bytes[header_end + 2..];
    if !pack.starts_with(b"PACK") {
        return Err(ShakoError::BadRequest("bundle contains no PACK".into()));
    }

    let pack_dir = repo_path.join("objects/pack");
    std::fs::create_dir_all(&pack_dir)
        .map_err(|e| ShakoError::Storage(format!("mkdir pack dir: {e}")))?;
    let mut cursor = std::io::Cursor::new(pack);
    let mut progress = gix_features::progress::Discard;
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
    .map_err(|e| ShakoError::Git(format!("restore bundle index-pack failed: {e}")))?;
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
        .map_err(|e| ShakoError::Git(format!("bundle rev_walk failed: {e}")))?;
    for item in walk.flatten() {
        set.insert(item.id);
        if let Ok(commit) = item.object()
            && let Ok(tree) = commit.tree()
        {
            collect_all_tree_objects(repo, &tree, &mut set)?;
        }
    }
    Ok(set.into_iter().collect())
}

fn collect_all_tree_objects(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    out: &mut BTreeSet<ObjectId>,
) -> ShakoResult<()> {
    if !out.insert(tree.id) {
        return Ok(());
    }
    let decoded = tree
        .decode()
        .map_err(|e| ShakoError::Git(format!("decode tree: {e}")))?;
    for entry in decoded.entries {
        if entry.mode.is_tree() {
            let sub = repo
                .find_tree(entry.oid)
                .map_err(|e| ShakoError::Git(format!("find subtree: {e}")))?;
            collect_all_tree_objects(repo, &sub, out)?;
        } else if !entry.mode.is_commit() {
            out.insert(entry.oid.to_owned());
        }
    }
    Ok(())
}
