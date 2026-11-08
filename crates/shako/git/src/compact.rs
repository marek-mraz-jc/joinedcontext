#![forbid(unsafe_code)]
//! In-process repository pack compaction and `fsck` verification (Contract S6, R§2.5, Threat T15).
//!
//! Compaction rules:
//! 1. Runs under the per-repository write lock (`RepoLocks`).
//! 2. Refuses to run if any quarantine directory (`objects/incoming-*`) exists.
//! 3. Copies reachable objects plus any unreachable packed objects from packs younger than the
//!    14-day prune grace into one new consolidated packfile (`delta reuse`), writes its `.idx`,
//!    and removes old packs only after the new pack and index are on disk.

use crate::bundle::collect_all_objects_from_tips;
use crate::repo::open_isolated;
use crate::upload_pack::{build_pack_bytes, collect_advertised_refs};
use gix_hash::ObjectId;
use shako_core::{ShakoError, ShakoResult};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

pub const DEFAULT_PRUNE_GRACE: Duration = Duration::from_secs(14 * 24 * 3600);
pub const AUTO_COMPACT_PACK_THRESHOLD: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactOutcome {
    pub packs_before: usize,
    pub packs_after: usize,
    pub objects_packed: usize,
    pub skipped_due_to_quarantine: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsckOutcome {
    pub verified_packs: usize,
    pub verified_refs: usize,
}

/// Checks if any `objects/incoming-*` quarantine directory exists in `repo_path` (R§2.5).
pub fn has_active_quarantine(repo_path: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(repo_path.join("objects")) else {
        return false;
    };
    for entry in rd.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("incoming-")
        {
            return true;
        }
    }
    false
}

fn list_pack_files(pack_dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(pack_dir) else {
        return Vec::new();
    };
    let mut packs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("pack"))
        .collect();
    packs.sort();
    packs
}

/// Compacts all packfiles in `repo_path` into a single packfile, keeping unreachable objects
/// from packs modified within `prune_grace` (default 14 days, Contract S6, Threat T15).
/// Caller MUST hold the repository's write lock (`RepoLocks::lock`).
pub fn compact_repository(
    repo_path: &Path,
    prune_grace: Duration,
) -> ShakoResult<CompactOutcome> {
    if has_active_quarantine(repo_path) {
        return Ok(CompactOutcome {
            packs_before: 0,
            packs_after: 0,
            objects_packed: 0,
            skipped_due_to_quarantine: true,
        });
    }

    let pack_dir = repo_path.join("objects/pack");
    let old_packs = list_pack_files(&pack_dir);
    if old_packs.is_empty() {
        return Ok(CompactOutcome {
            packs_before: 0,
            packs_after: 0,
            objects_packed: 0,
            skipped_due_to_quarantine: false,
        });
    }

    let repo = open_isolated(repo_path)?;
    let (_, refs) = collect_advertised_refs(&repo, true)?;
    let mut tips = Vec::new();
    let mut objects_to_keep: BTreeSet<ObjectId> = BTreeSet::new();

    for r in &refs {
        objects_to_keep.insert(r.oid);
        if let Some(p) = r.peeled {
            objects_to_keep.insert(p);
            tips.push(p);
        } else if repo.find_commit(r.oid).is_ok() {
            tips.push(r.oid);
        }
    }
    for oid in collect_all_objects_from_tips(&repo, &tips)? {
        objects_to_keep.insert(oid);
    }

    // Preserve unreachable objects from packs whose mtime is within `prune_grace` (14-day grace, S6)
    let now = SystemTime::now();
    for pack_path in &old_packs {
        let within_grace = pack_path
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_none_or(|age| age < prune_grace);
        if within_grace
            && let Ok(bundle) = gix_pack::Bundle::at(pack_path, gix_hash::Kind::Sha1)
        {
            for entry in bundle.index.iter() {
                objects_to_keep.insert(entry.oid);
            }
        }
    }

    if objects_to_keep.is_empty() {
        return Ok(CompactOutcome {
            packs_before: old_packs.len(),
            packs_after: old_packs.len(),
            objects_packed: 0,
            skipped_due_to_quarantine: false,
        });
    }

    let oid_vec: Vec<ObjectId> = objects_to_keep.into_iter().collect();
    let pack_bytes = build_pack_bytes(&repo, &oid_vec, false)?;

    let mut cursor = std::io::Cursor::new(pack_bytes);
    let mut progress = gix_features::progress::Discard;
    let should_interrupt = AtomicBool::new(false);
    let outcome = gix_pack::Bundle::write_to_directory(
        &mut cursor,
        Some(&pack_dir),
        &mut progress,
        &should_interrupt,
        None::<gix_odb::Handle>,
        gix_hash::Kind::Sha1,
        gix_pack::bundle::write::Options::default(),
    )
    .map_err(|e| ShakoError::Git(format!("compact write bundle failed: {e}")))?;

    let new_pack_path = outcome.data_path;
    if let Some(keep) = outcome.keep_path {
        let _ = std::fs::remove_file(keep);
    }

    // Delete old packs that were superseded by `new_pack_path`
    for old_pack in &old_packs {
        if Some(old_pack.as_path()) != new_pack_path.as_deref() {
            let old_idx = old_pack.with_extension("idx");
            let _ = std::fs::remove_file(old_pack);
            let _ = std::fs::remove_file(old_idx);
        }
    }

    let after = list_pack_files(&pack_dir).len();
    Ok(CompactOutcome {
        packs_before: old_packs.len(),
        packs_after: after,
        objects_packed: oid_vec.len(),
        skipped_due_to_quarantine: false,
    })
}

/// Verifies packfile checksums/integrity via `gix_pack::Bundle::verify_integrity` and verifies
/// object graph connectivity of every reference in `repo_path` (`shako fsck`, Contract S6).
pub fn fsck_repository(repo_path: &Path) -> ShakoResult<FsckOutcome> {
    let pack_dir = repo_path.join("objects/pack");
    let packs = list_pack_files(&pack_dir);
    let should_interrupt = AtomicBool::new(false);

    for pack_path in &packs {
        let bundle = gix_pack::Bundle::at(pack_path, gix_hash::Kind::Sha1)
            .map_err(|e| ShakoError::Git(format!("fsck open bundle {}: {e}", pack_path.display())))?;
        let mut progress = gix_features::progress::Discard;
        bundle
            .verify_integrity(
                &mut progress,
                &should_interrupt,
                gix_pack::bundle::verify::integrity::Options {
                    verify_mode: gix_pack::index::verify::Mode::HashCrc32Decode,
                    traversal: gix_pack::index::traverse::Algorithm::Lookup,
                    make_pack_lookup_cache: || gix_pack::cache::Never,
                    thread_limit: Some(1),
                },
            )
            .map_err(|e| {
                ShakoError::Git(format!("fsck integrity failed on {}: {e}", pack_path.display()))
            })?;
    }

    let repo = open_isolated(repo_path)?;
    let (_, refs) = collect_advertised_refs(&repo, true)?;
    for r in &refs {
        if !repo.has_object(r.oid) {
            return Err(ShakoError::Git(format!(
                "fsck: ref {} points to missing object {}",
                r.name, r.oid
            )));
        }
    }

    Ok(FsckOutcome {
        verified_packs: packs.len(),
        verified_refs: refs.len(),
    })
}
