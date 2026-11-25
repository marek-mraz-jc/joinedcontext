#![forbid(unsafe_code)]
//! Repository archive generation (`.zip` and `.tar.gz`, Row 32, Contract S11, Threat T1).
//! Prefix is `{repo}-{ref}/`, modification time is the commit timestamp.

use crate::git_err;
use crate::pack::{Admit, job_bytes};
use crate::repo::{collect_tree_blobs, open_isolated, resolve_commit_oid};
use flate2::Compression;
use flate2::write::GzEncoder;
use gix_object::tree::EntryKind;
use shako_core::{ShakoError, ShakoResult, validate_repo_or_owner_name, validate_tree_path};
use std::collections::BTreeMap;
use std::io::{Seek, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}

fn sanitize_archive_prefix_ref(git_ref: &str) -> ShakoResult<String> {
    let cleaned = git_ref.trim().replace('/', "-");
    if cleaned.is_empty()
        || cleaned.contains("..")
        || cleaned.contains('\\')
        || cleaned.contains('\0')
    {
        return Err(ShakoError::Unprocessable("invalid archive ref".into()));
    }
    Ok(cleaned)
}

/// Generates a `.zip` or `.tar.gz` archive of `git_ref` with prefix `{repo_name}-{ref}/`
/// and entry modification time set to the commit's timestamp (Contract S11).
pub fn create_archive(
    repo_path: &Path,
    repo_name: &str,
    git_ref: &str,
    format: ArchiveFormat,
    out: impl Write + Seek,
    admit: Admit<'_>,
) -> ShakoResult<()> {
    validate_repo_or_owner_name(repo_name, "repository")?;
    let safe_ref = sanitize_archive_prefix_ref(git_ref)?;
    let prefix = format!("{repo_name}-{safe_ref}");

    let repo = open_isolated(repo_path)?;
    let commit_oid = resolve_commit_oid(&repo, git_ref)?;
    let commit = repo
        .find_commit(commit_oid)
        .map_err(git_err("find commit"))?;
    let mtime_secs = commit
        .time()
        .map(|t| u64::try_from(t.seconds.max(0)).unwrap_or(0))
        .unwrap_or(0);
    let tree = commit.tree().map_err(git_err("find tree"))?;

    let mut files = BTreeMap::new();
    collect_tree_blobs(&repo, &tree, "", &mut files)?;
    // A submodule's content is in another repository; the archive leaves it out, as git does.
    files.retain(|_, (kind, _)| *kind != EntryKind::Commit);

    let mut largest = 0;
    for (_, blob_oid) in files.values() {
        let header = repo.find_header(*blob_oid).map_err(git_err("find blob"))?;
        largest = largest.max(header.size());
    }
    admit(job_bytes(largest, files.len() as u64))?;

    match format {
        ArchiveFormat::Zip => {
            {
                let mut zip = zip::ZipWriter::new(out);
                for (rel_path, (kind, blob_oid)) in &files {
                    validate_tree_path(rel_path)?;
                    let blob = repo.find_blob(*blob_oid).map_err(git_err("find blob"))?;
                    let mode = if *kind == EntryKind::BlobExecutable {
                        0o755
                    } else {
                        0o644
                    };
                    let opts = SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated)
                        .unix_permissions(mode);
                    let entry_name = format!("{prefix}/{rel_path}");
                    zip.start_file(entry_name, opts)
                        .map_err(git_err("zip start_file"))?;
                    zip.write_all(&blob.data).map_err(git_err("zip write"))?;
                }
                zip.finish().map_err(git_err("zip finish"))?;
            }
            Ok(())
        }
        ArchiveFormat::TarGz => {
            let mut gz = GzEncoder::new(out, Compression::default());
            {
                let mut builder = tar::Builder::new(&mut gz);
                for (rel_path, (kind, blob_oid)) in &files {
                    validate_tree_path(rel_path)?;
                    let blob = repo.find_blob(*blob_oid).map_err(git_err("find blob"))?;
                    let mut header = tar::Header::new_gnu();
                    header.set_size(blob.data.len() as u64);
                    header.set_mtime(mtime_secs);
                    header.set_mode(if *kind == EntryKind::BlobExecutable {
                        0o755
                    } else {
                        0o644
                    });
                    header.set_cksum();
                    let entry_name = format!("{prefix}/{rel_path}");
                    builder
                        .append_data(&mut header, entry_name, blob.data.as_slice())
                        .map_err(git_err("tar append"))?;
                }
                builder.finish().map_err(git_err("tar finish"))?;
            }
            gz.finish().map(drop).map_err(git_err("gzip finish"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::set_ref_direct;
    use crate::testutil::repo;
    use std::io::{Cursor, Read as _};

    /// A commit on `feature/x` holding `README.md`, an executable `run.sh` and a submodule.
    fn fixture(dir: &Path) -> std::path::PathBuf {
        let path = repo(dir, 1, false);
        let handle = open_isolated(&path).expect("open");
        let readme = handle.write_blob(b"# readme\n").expect("blob").detach();
        let script = handle.write_blob(b"#!/bin/sh\n").expect("blob").detach();
        let sub =
            gix_hash::ObjectId::from_hex(b"1111111111111111111111111111111111111111").expect("id");
        let entry = |name: &str, mode: EntryKind, oid| gix_object::tree::Entry {
            mode: mode.into(),
            filename: name.into(),
            oid,
        };
        let tree = gix_object::Tree {
            entries: vec![
                entry("README.md", EntryKind::Blob, readme),
                entry("run.sh", EntryKind::BlobExecutable, script),
                entry("vendor", EntryKind::Commit, sub),
            ],
        };
        let tree = handle.write_object(&tree).expect("tree").detach();
        let sig = gix::actor::Signature {
            name: "a".into(),
            email: "a@example.org".into(),
            time: gix::date::Time::new(1_700_000_000, 0),
        };
        let commit = gix_object::Commit {
            tree,
            parents: Default::default(),
            author: sig.clone(),
            committer: sig,
            encoding: None,
            message: "m".into(),
            extra_headers: Vec::new(),
        };
        let commit = handle.write_object(&commit).expect("commit").detach();
        set_ref_direct(&path, "refs/heads/feature/x", &commit.to_string()).expect("ref");
        path
    }

    fn archive(
        path: &Path,
        name: &str,
        git_ref: &str,
        format: ArchiveFormat,
    ) -> ShakoResult<Vec<u8>> {
        let mut out = Cursor::new(Vec::new());
        create_archive(
            path,
            name,
            git_ref,
            format,
            &mut out,
            &mut crate::testutil::admit_all,
        )?;
        Ok(out.into_inner())
    }

    #[test]
    fn a_zip_holds_each_file_under_the_prefix_with_its_mode_and_no_submodule() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path());
        let bytes = archive(&path, "dest", " feature/x ", ArchiveFormat::Zip).expect("zip");
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("a zip");
        let mut names = Vec::new();
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).expect("entry");
            names.push((
                file.name().expect("name").into_owned(),
                file.unix_mode().map(|m| m & 0o777),
            ));
            let mut body = String::new();
            file.read_to_string(&mut body).expect("read");
            assert!(body.starts_with('#'), "{body}");
        }
        assert_eq!(
            names,
            [
                ("dest-feature-x/README.md".to_owned(), Some(0o644)),
                ("dest-feature-x/run.sh".to_owned(), Some(0o755)),
            ]
        );
    }

    #[test]
    fn a_tar_gz_holds_each_file_with_its_mode_and_the_commit_time() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path());
        let bytes = archive(&path, "dest", "feature/x", ArchiveFormat::TarGz).expect("tar.gz");
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(bytes)));
        let entries: Vec<_> = tar
            .entries()
            .expect("entries")
            .map(|e| {
                let e = e.expect("entry");
                let header = e.header();
                (
                    e.path().expect("path").to_string_lossy().into_owned(),
                    header.mode().expect("mode"),
                    header.mtime().expect("mtime"),
                )
            })
            .collect();
        assert_eq!(
            entries,
            [
                ("dest-feature-x/README.md".to_owned(), 0o644, 1_700_000_000),
                ("dest-feature-x/run.sh".to_owned(), 0o755, 1_700_000_000),
            ]
        );
    }

    #[test]
    fn an_archive_refuses_a_bad_name_or_ref_an_unknown_ref_a_refusal_and_a_failed_write() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path());
        assert!(archive(&path, "../x", "feature/x", ArchiveFormat::Zip).is_err());
        for bad in ["", "  ", "a..b", "a\\b", "a\0b"] {
            let err = archive(&path, "dest", bad, ArchiveFormat::Zip).expect_err(bad);
            assert!(
                err.to_string().contains("invalid archive ref"),
                "{bad:?}: {err}"
            );
        }
        let err = archive(&path, "dest", "nope", ArchiveFormat::TarGz).expect_err("unknown");
        assert_eq!(err.status_code(), 404);
        let err = create_archive(
            &path,
            "dest",
            "feature/x",
            ArchiveFormat::Zip,
            Cursor::new(Vec::new()),
            &mut |_| Err(ShakoError::Overloaded("busy".into())),
        )
        .expect_err("refused");
        assert!(matches!(err, ShakoError::Overloaded(_)));
        for format in [ArchiveFormat::Zip, ArchiveFormat::TarGz] {
            // A writer with no room: every write fails, seeking works.
            let mut room = [0u8; 0];
            let full = Cursor::new(&mut room[..]);
            let err = create_archive(
                &path,
                "dest",
                "feature/x",
                format,
                full,
                &mut crate::testutil::admit_all,
            )
            .expect_err("no room");
            assert!(matches!(err, ShakoError::Git(_)), "{format:?}: {err}");
        }
    }
}
