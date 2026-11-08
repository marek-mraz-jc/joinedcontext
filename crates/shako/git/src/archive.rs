#![forbid(unsafe_code)]
//! Repository archive generation (`.zip` and `.tar.gz`, Row 32, Contract S11, Threat T1).
//! Prefix is `{repo}-{ref}/`, modification time is the commit timestamp.

use crate::repo::{collect_tree_blobs, open_isolated, resolve_commit_oid};
use flate2::Compression;
use flate2::write::GzEncoder;
use gix_object::tree::EntryKind;
use shako_core::{ShakoError, ShakoResult, validate_repo_or_owner_name, validate_tree_path};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use zip::write::SimpleFileOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}

fn sanitize_archive_prefix_ref(git_ref: &str) -> ShakoResult<String> {
    let cleaned = git_ref.trim().replace('/', "-");
    if cleaned.is_empty() || cleaned.contains("..") || cleaned.contains('\\') || cleaned.contains('\0') {
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
) -> ShakoResult<Vec<u8>> {
    validate_repo_or_owner_name(repo_name, "repository")?;
    let safe_ref = sanitize_archive_prefix_ref(git_ref)?;
    let prefix = format!("{repo_name}-{safe_ref}");

    let repo = open_isolated(repo_path)?;
    let commit_oid = resolve_commit_oid(&repo, git_ref)?;
    let commit = repo
        .find_commit(commit_oid)
        .map_err(|e| ShakoError::Git(format!("find commit: {e}")))?;
    let mtime_secs = commit
        .time()
        .map(|t| u64::try_from(t.seconds.max(0)).unwrap_or(0))
        .unwrap_or(0);
    let tree = commit
        .tree()
        .map_err(|e| ShakoError::Git(format!("find tree: {e}")))?;

    let mut files = BTreeMap::new();
    collect_tree_blobs(&repo, &tree, "", &mut files)?;

    match format {
        ArchiveFormat::Zip => {
            let mut cursor = std::io::Cursor::new(Vec::new());
            {
                let mut zip = zip::ZipWriter::new(&mut cursor);
                for (rel_path, (kind, blob_oid)) in &files {
                    validate_tree_path(rel_path)?;
                    let blob = repo
                        .find_blob(*blob_oid)
                        .map_err(|e| ShakoError::Git(format!("find blob: {e}")))?;
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
                        .map_err(|e| ShakoError::Git(format!("zip start_file: {e}")))?;
                    zip.write_all(&blob.data)
                        .map_err(|e| ShakoError::Git(format!("zip write: {e}")))?;
                }
                zip.finish()
                    .map_err(|e| ShakoError::Git(format!("zip finish: {e}")))?;
            }
            Ok(cursor.into_inner())
        }
        ArchiveFormat::TarGz => {
            let mut gz = GzEncoder::new(Vec::new(), Compression::default());
            {
                let mut builder = tar::Builder::new(&mut gz);
                for (rel_path, (kind, blob_oid)) in &files {
                    validate_tree_path(rel_path)?;
                    let blob = repo
                        .find_blob(*blob_oid)
                        .map_err(|e| ShakoError::Git(format!("find blob: {e}")))?;
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
                        .map_err(|e| ShakoError::Git(format!("tar append: {e}")))?;
                }
                builder
                    .finish()
                    .map_err(|e| ShakoError::Git(format!("tar finish: {e}")))?;
            }
            gz.finish()
                .map_err(|e| ShakoError::Git(format!("gzip finish: {e}")))
        }
    }
}
