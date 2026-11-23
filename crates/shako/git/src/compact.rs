#![forbid(unsafe_code)]
//! In-process repository pack compaction and `fsck` verification (Contract S6, R§2.5, Threat T15).
//!
//! Compaction rules:
//! 1. Runs under the per-repository write lock (`RepoLocks`).
//! 2. Refuses to run while a quarantine directory (`objects/incoming-*`) younger than a day
//!    exists; an older one is a crashed push's and is removed.
//! 3. Copies reachable objects into one new packfile (`delta reuse`) and the unreachable objects
//!    first seen within the 14-day prune grace into a cruft pack, whose `.seen` file keeps each
//!    object's first-seen time, so compacting again never renews the grace (S6-42). Old packs go
//!    only after the new packs, their indexes and the `.seen` file are on disk.
//! 4. Runs by itself after a write once a repository holds `AUTO_COMPACT_PACK_THRESHOLD` packs
//!    (`compaction_due`).

use crate::bundle::{collect_all_objects_from_tips, collect_all_tree_objects};
use crate::pack::{Admit, write_pack};
use crate::repo::open_isolated;
use crate::upload_pack::collect_advertised_refs;
use gix_hash::ObjectId;
use shako_core::{ShakoError, ShakoResult};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_PRUNE_GRACE: Duration = Duration::from_secs(14 * 24 * 3600);
/// A write that leaves this many packs compacts the repository (each API commit, merge and push
/// adds one).
pub const AUTO_COMPACT_PACK_THRESHOLD: usize = 16;
/// Beside a cruft pack: `<hex id> <unix seconds>` per line, when each object was first seen.
const SEEN: &str = "seen";
/// How long a superseded pack stays for readers that loaded its index: a fetch's transfer limit.
const SUPERSEDED_KEPT: Duration = Duration::from_secs(3600);
/// A push lives minutes (30 for its transfer); a quarantine this old was left by a crash.
const STALE_QUARANTINE: Duration = Duration::from_secs(24 * 3600);

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
        if entry.file_name().to_string_lossy().starts_with("incoming-") {
            return true;
        }
    }
    false
}

/// Removes the quarantines crashed pushes left. One that cannot be read or removed stays and
/// holds compaction back.
fn remove_stale_quarantines(repo_path: &Path) {
    let Ok(rd) = std::fs::read_dir(repo_path.join("objects")) else {
        return;
    };
    for entry in rd.flatten() {
        let stale = entry.file_name().to_string_lossy().starts_with("incoming-")
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| at.elapsed().ok())
                .is_some_and(|age| age >= STALE_QUARANTINE);
        if stale {
            // One left standing shows in the outcome, as `skipped_due_to_quarantine`.
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The packs git serves: those with an index. A pack compaction superseded has none.
fn list_pack_files(pack_dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(pack_dir) else {
        return Vec::new();
    };
    let mut packs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("pack"))
        .filter(|p| p.with_extension("idx").is_file())
        .collect();
    packs.sort();
    packs
}

/// Takes `pack` out of what git reads: its index first, so no new reader finds it, while a
/// reader that loaded the index before can still open the pack (`sweep_superseded` removes it
/// after `SUPERSEDED_KEPT`). Its mtime marks when.
fn supersede(pack: &Path) {
    let _ = std::fs::remove_file(pack.with_extension("idx"));
    let _ = std::fs::remove_file(pack.with_extension(SEEN));
    let _ = std::fs::File::options()
        .write(true)
        .open(pack)
        .and_then(|f| f.set_modified(SystemTime::now()));
}

/// Removes superseded packs older than `SUPERSEDED_KEPT`, and an index or `.seen` file whose
/// pack a crash removed.
fn sweep_superseded(pack_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(pack_dir) else {
        return;
    };
    for path in rd.flatten().map(|e| e.path()) {
        let ext = path.extension().and_then(|s| s.to_str());
        let stale = match ext {
            Some("pack") => {
                !path.with_extension("idx").exists()
                    && path
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|at| at.elapsed().ok())
                        .is_some_and(|age| age >= SUPERSEDED_KEPT)
            }
            Some("idx" | SEEN) => !path.with_extension("pack").exists(),
            _ => false,
        };
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether `repo_path` holds enough packs that the next write should compact it.
pub fn compaction_due(repo_path: &Path) -> bool {
    list_pack_files(&repo_path.join("objects/pack")).len() >= AUTO_COMPACT_PACK_THRESHOLD
}

/// The first-seen times a cruft pack's `.seen` file holds; a line that does not parse is left
/// out, so its object counts from the pack's own mtime (kept longer, never lost early).
fn read_seen(pack_path: &Path) -> HashMap<ObjectId, SystemTime> {
    let Ok(text) = std::fs::read_to_string(pack_path.with_extension(SEEN)) else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let (id, secs) = line.split_once(' ')?;
            let id = ObjectId::from_hex(id.as_bytes()).ok()?;
            Some((id, UNIX_EPOCH + Duration::from_secs(secs.parse().ok()?)))
        })
        .collect()
}

/// Writes `ids` as one pack and index into `pack_dir`; answers the pack's path.
fn write_bundle(
    repo: &gix::Repository,
    ids: &[ObjectId],
    pack_dir: &Path,
    admit: Admit<'_>,
) -> ShakoResult<Option<PathBuf>> {
    let spooled = |e: std::io::Error| ShakoError::Storage(format!("compact spool: {e}"));
    let mut spool = tempfile::tempfile_in(pack_dir).map_err(spooled)?;
    write_pack(repo, ids, &mut spool, admit)?;
    spool.rewind().map_err(spooled)?;
    let mut reader = std::io::BufReader::new(spool);
    let mut progress = gix::progress::Discard;
    let should_interrupt = AtomicBool::new(false);
    let outcome = gix_pack::Bundle::write_to_directory(
        &mut reader,
        Some(pack_dir),
        &mut progress,
        &should_interrupt,
        None::<gix_odb::Handle>,
        gix_hash::Kind::Sha1,
        gix_pack::bundle::write::Options {
            thread_limit: Some(1),
            ..Default::default()
        },
    )
    .map_err(|e| ShakoError::Git(format!("compact write bundle failed: {e}")))?;
    if let Some(keep) = outcome.keep_path {
        let _ = std::fs::remove_file(keep);
    }
    if let Some(pack) = &outcome.data_path {
        for file in [pack.clone(), pack.with_extension("idx")] {
            std::fs::File::open(&file)
                .and_then(|f| f.sync_all())
                .map_err(|e| ShakoError::Storage(format!("compact sync: {e}")))?;
        }
    }
    Ok(outcome.data_path)
}

/// Writes `seen` beside `pack` (temporary file, then rename). Refused, the pack goes again, so
/// no cruft pack stands without its times.
fn write_seen(pack: &Path, seen: &BTreeMap<ObjectId, SystemTime>) -> ShakoResult<()> {
    let written = (|| {
        let mut file = tempfile::NamedTempFile::new_in(pack.parent().unwrap_or(pack))?;
        for (id, at) in seen {
            let secs = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            writeln!(file, "{id} {secs}")?;
        }
        file.as_file().sync_all()?;
        file.persist(pack.with_extension(SEEN))
            .map_err(|e| e.error)
            .map(|_| ())
    })();
    written.map_err(|e| {
        let _ = std::fs::remove_file(pack.with_extension("idx"));
        let _ = std::fs::remove_file(pack);
        ShakoError::Storage(format!("compact seen times: {e}"))
    })
}

/// Compacts all packfiles in `repo_path` into a single packfile, keeping unreachable objects
/// from packs modified within `prune_grace` (default 14 days, Contract S6, Threat T15).
/// Caller MUST hold the repository's write lock (`RepoLocks::lock`).
pub fn compact_repository(
    repo_path: &Path,
    prune_grace: Duration,
    admit: Admit<'_>,
) -> ShakoResult<CompactOutcome> {
    remove_stale_quarantines(repo_path);
    if has_active_quarantine(repo_path) {
        return Ok(CompactOutcome {
            packs_before: 0,
            packs_after: 0,
            objects_packed: 0,
            skipped_due_to_quarantine: true,
        });
    }

    let pack_dir = repo_path.join("objects/pack");
    sweep_superseded(&pack_dir);
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

    // Every ref peeled to its end: each tag of a chain, and a tree or blob a tag names.
    let unreadable = |e: &dyn std::fmt::Display| ShakoError::Git(format!("compact walk: {e}"));
    for r in &refs {
        let mut at = r.oid;
        loop {
            let object = repo.find_object(at).map_err(|e| unreadable(&e))?;
            match object.kind {
                gix_object::Kind::Tag => {
                    objects_to_keep.insert(at);
                    let tag = object.try_into_tag().map_err(|e| unreadable(&e))?;
                    at = tag.target_id().map_err(|e| unreadable(&e))?.detach();
                }
                gix_object::Kind::Commit => {
                    tips.push(at);
                    break;
                }
                gix_object::Kind::Tree => {
                    let tree = object.try_into_tree().map_err(|e| unreadable(&e))?;
                    collect_all_tree_objects(&repo, &tree, &mut objects_to_keep)?;
                    break;
                }
                gix_object::Kind::Blob => {
                    objects_to_keep.insert(at);
                    break;
                }
            }
        }
    }
    for oid in collect_all_objects_from_tips(&repo, &tips)? {
        objects_to_keep.insert(oid);
    }

    // Unreachable objects first seen within `prune_grace` (S6) go to a cruft pack with their
    // times; a pack's mtime stands for the objects its `.seen` file does not name.
    let now = SystemTime::now();
    let age = |at: SystemTime| now.duration_since(at).unwrap_or_default();
    let mut cruft: BTreeMap<ObjectId, SystemTime> = BTreeMap::new();
    for pack_path in &old_packs {
        let mtime = pack_path
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(now);
        if age(mtime) >= prune_grace {
            continue;
        }
        let Ok(bundle) = gix_pack::Bundle::at(pack_path, gix_hash::Kind::Sha1) else {
            continue;
        };
        let seen = read_seen(pack_path);
        for entry in bundle.index.iter() {
            if objects_to_keep.contains(&entry.oid) {
                continue;
            }
            let at = seen.get(&entry.oid).map_or(mtime, |t| (*t).min(mtime));
            if age(at) < prune_grace {
                let held = cruft.entry(entry.oid).or_insert(at);
                *held = (*held).max(at);
            }
        }
    }

    if objects_to_keep.is_empty() && cruft.is_empty() {
        return Ok(CompactOutcome {
            packs_before: old_packs.len(),
            packs_after: old_packs.len(),
            objects_packed: 0,
            skipped_due_to_quarantine: false,
        });
    }

    let reachable: Vec<ObjectId> = objects_to_keep.into_iter().collect();
    let unreachable: Vec<ObjectId> = cruft.keys().copied().collect();
    let mut written = Vec::new();
    if !reachable.is_empty() {
        written.extend(write_bundle(&repo, &reachable, &pack_dir, &mut *admit)?);
    }
    if !unreachable.is_empty()
        && let Some(pack) = write_bundle(&repo, &unreachable, &pack_dir, &mut *admit)?
    {
        write_seen(&pack, &cruft)?;
        written.push(pack);
    }

    // The new packs reach the disk before an old one leaves what git reads.
    std::fs::File::open(&pack_dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| ShakoError::Storage(format!("compact sync: {e}")))?;
    for old_pack in &old_packs {
        if !written.contains(old_pack) {
            supersede(old_pack);
        }
    }

    let after = list_pack_files(&pack_dir).len();
    Ok(CompactOutcome {
        packs_before: old_packs.len(),
        packs_after: after,
        objects_packed: reachable.len() + unreachable.len(),
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
        let bundle = gix_pack::Bundle::at(pack_path, gix_hash::Kind::Sha1).map_err(|e| {
            ShakoError::Git(format!("fsck open bundle {}: {e}", pack_path.display()))
        })?;
        let mut progress = gix::progress::Discard;
        bundle
            .verify_integrity(
                &mut progress,
                &should_interrupt,
                gix_pack::index::verify::integrity::Options {
                    verify_mode: gix_pack::index::verify::Mode::HashCrc32Decode,
                    traversal: gix_pack::index::traverse::Algorithm::Lookup,
                    make_pack_lookup_cache: || gix_pack::cache::Never,
                    thread_limit: Some(1),
                },
            )
            .map_err(|e| {
                ShakoError::Git(format!(
                    "fsck integrity failed on {}: {e}",
                    pack_path.display()
                ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{init_bare_repo, write_objects_as_pack};
    use shako_core::{InstanceId, RepoId};

    const DAY: Duration = Duration::from_secs(24 * 3600);

    /// Writes `blob` as a pack of its own, `age` old; answers its id.
    fn orphan(repo: &Path, blob: &[u8], age: Duration) -> ObjectId {
        let id = gix_object::compute_hash(gix_hash::Kind::Sha1, gix_object::Kind::Blob, blob)
            .expect("hash");
        let before = list_pack_files(&repo.join("objects/pack"));
        write_objects_as_pack(repo, &[(id, gix_object::Kind::Blob, blob.to_vec())]).expect("pack");
        for pack in list_pack_files(&repo.join("objects/pack")) {
            if !before.contains(&pack) {
                std::fs::File::options()
                    .write(true)
                    .open(&pack)
                    .and_then(|f| f.set_modified(SystemTime::now() - age))
                    .expect("age pack");
            }
        }
        id
    }

    fn held(repo: &Path, id: ObjectId) -> bool {
        list_pack_files(&repo.join("objects/pack")).iter().any(|p| {
            gix_pack::Bundle::at(p, gix_hash::Kind::Sha1)
                .is_ok_and(|b| b.index.lookup(id).is_some())
        })
    }

    #[test]
    fn compacting_again_keeps_an_unreachable_objects_first_seen_time() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = init_bare_repo(
            tmp.path(),
            InstanceId::from_u128(1),
            RepoId::from_u128(7),
            "main",
            true,
            "grace",
        )
        .expect("init");
        let old = orphan(&repo, b"pushed away 13 days ago\n", 13 * DAY);
        let new = orphan(
            &repo,
            b"pushed away an hour ago\n",
            Duration::from_secs(3600),
        );
        let gone = orphan(&repo, b"pushed away 15 days ago\n", 15 * DAY);
        let compact = |grace| compact_repository(&repo, grace, &mut |_| Ok(())).expect("compact");

        compact(14 * DAY);
        assert!(
            held(&repo, old) && held(&repo, new),
            "within the grace: kept"
        );
        assert!(!held(&repo, gone), "past the grace: pruned");
        assert_eq!(
            compact(14 * DAY).packs_after,
            2,
            "one reachable pack, one cruft pack"
        );
        // Seen 13 days ago, so a 12-day grace prunes it although its pack was written just now.
        compact(12 * DAY);
        assert!(!held(&repo, old), "compaction renewed the grace");
        assert!(held(&repo, new));
        assert!(fsck_repository(&repo).is_ok());
    }

    fn object(kind: gix_object::Kind, raw: Vec<u8>) -> (ObjectId, gix_object::Kind, Vec<u8>) {
        let id = gix_object::compute_hash(gix_hash::Kind::Sha1, kind, &raw).expect("hash");
        (id, kind, raw)
    }

    fn tag(target: ObjectId, kind: &str, name: &str) -> (ObjectId, gix_object::Kind, Vec<u8>) {
        let raw = format!(
            "object {target}\ntype {kind}\ntag {name}\ntagger T <t@example.org> 0 +0000\n\n{name}\n"
        );
        object(gix_object::Kind::Tag, raw.into_bytes())
    }

    #[test]
    fn a_tag_chain_that_names_a_tree_keeps_the_tree_and_its_blobs() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = init_bare_repo(
            tmp.path(),
            InstanceId::from_u128(1),
            RepoId::from_u128(9),
            "main",
            true,
            "tags",
        )
        .expect("init");
        let blob = object(gix_object::Kind::Blob, b"behind a tag\n".to_vec());
        let mut tree_raw = b"100644 kept.txt\0".to_vec();
        tree_raw.extend_from_slice(blob.0.as_bytes());
        let tree = object(gix_object::Kind::Tree, tree_raw);
        let inner = tag(tree.0, "tree", "inner");
        let outer = tag(inner.0, "tag", "outer");
        let ids = [blob.0, tree.0, inner.0, outer.0];
        write_objects_as_pack(&repo, &[blob, tree, inner, outer]).expect("pack");
        crate::repo::set_ref_direct(&repo, "refs/tags/outer", &ids[3].to_string()).expect("ref");
        // A zero grace prunes every object the walk misses.
        compact_repository(&repo, Duration::ZERO, &mut |_| Ok(())).expect("compact");
        for id in ids {
            assert!(held(&repo, id), "{id} lost");
        }
    }

    #[test]
    fn a_superseded_pack_stays_for_an_hour_for_readers_then_goes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = init_bare_repo(
            tmp.path(),
            InstanceId::from_u128(1),
            RepoId::from_u128(10),
            "main",
            true,
            "sweep",
        )
        .expect("init");
        let pack_dir = repo.join("objects/pack");
        // Two packs become one cruft pack, so both old names leave.
        orphan(&repo, b"one\n", Duration::ZERO);
        orphan(&repo, b"two\n", Duration::ZERO);
        let before = list_pack_files(&pack_dir);
        compact_repository(&repo, DAY, &mut |_| Ok(())).expect("compact");
        let superseded: Vec<_> = before
            .iter()
            .filter(|p| !list_pack_files(&pack_dir).contains(p))
            .cloned()
            .collect();
        assert!(!superseded.is_empty());
        for pack in &superseded {
            assert!(pack.exists() && !pack.with_extension("idx").exists());
        }
        // A crash left an index without its pack.
        let orphan_idx = pack_dir.join("pack-crashed.idx");
        std::fs::write(&orphan_idx, b"x").expect("orphan idx");
        compact_repository(&repo, DAY, &mut |_| Ok(())).expect("compact");
        assert!(
            superseded.iter().all(|p| p.exists()),
            "removed within the hour"
        );
        assert!(!orphan_idx.exists());
        for pack in &superseded {
            std::fs::File::options()
                .write(true)
                .open(pack)
                .and_then(|f| f.set_modified(SystemTime::now() - SUPERSEDED_KEPT))
                .expect("age");
        }
        compact_repository(&repo, DAY, &mut |_| Ok(())).expect("compact");
        assert!(superseded.iter().all(|p| !p.exists()));
        assert!(fsck_repository(&repo).is_ok());
    }

    #[test]
    fn a_repository_is_due_once_it_holds_the_threshold_of_packs() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = init_bare_repo(
            tmp.path(),
            InstanceId::from_u128(1),
            RepoId::from_u128(8),
            "main",
            false,
            "due",
        )
        .expect("init");
        for n in 0..AUTO_COMPACT_PACK_THRESHOLD {
            assert!(!compaction_due(&repo), "due after {n} packs");
            orphan(&repo, format!("blob {n}\n").as_bytes(), Duration::ZERO);
        }
        assert!(compaction_due(&repo));
        compact_repository(&repo, DAY, &mut |_| Ok(())).expect("compact");
        assert!(!compaction_due(&repo));
    }

    #[test]
    fn a_seen_line_that_does_not_parse_counts_from_the_packs_mtime() {
        let tmp = tempfile::tempdir().expect("tmp");
        let pack = tmp.path().join("pack-x.pack");
        let id = ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567").expect("id");
        std::fs::write(
            pack.with_extension(SEEN),
            format!("{id} 1000\nnot a line\n{id}x 5\n"),
        )
        .expect("seen");
        let seen = read_seen(&pack);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[&id], UNIX_EPOCH + Duration::from_secs(1000));
    }
}
