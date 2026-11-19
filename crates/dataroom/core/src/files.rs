//! The files of an open data portal (T-7417): images and documents an Organization admin
//! uploads and uses in pages, the theme and the layout. The bytes sit on disk under the
//! Organization's own prefix, `{root}/{OrgId}/{CatalogueId}/`, named by their SHA-256, never in
//! the database; each catalogue keeps its index beside them. A file is served only on its own
//! catalogue's hosts (and an Organization-wide file on every portal of that Organization), at an
//! immutable address that names its hash: `/files/{sha256}/{name}`.
//!
//! The type comes from the bytes, never from the name or the declared type. An SVG passes the
//! logo's checks (no script, event handler, external reference or entity) and is still served as
//! an attachment; only raster images are shown inline.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{DataroomError, Result};
use crate::model::{CatalogueId, OrgId};

/// Largest single file, in bytes (10 MiB).
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// Most bytes one Organization holds over all its portals (200 MiB).
pub const MAX_ORG_BYTES: u64 = 200 * 1024 * 1024;
/// Most files one Organization holds over all its portals.
pub const MAX_ORG_FILES: usize = 500;
/// Longest display name, in characters.
pub const MAX_NAME_CHARS: usize = 120;

// ponytail: one limit class for every Organization; upgrade path: read the three caps from the
// Organization's limit class once Organizations carry one.

/// The limits of one Organization and what it holds now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileUsage {
    pub files: usize,
    pub bytes: u64,
    pub max_files: usize,
    pub max_bytes: u64,
    pub max_file_bytes: u64,
}

/// One stored file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileEntry {
    /// Stable id, kept across a rename or a replace.
    pub id: String,
    /// Display and download name, e.g. `hero.png`.
    pub name: String,
    /// From the bytes: `image/png`, `image/jpeg`, `image/webp`, `image/svg+xml`,
    /// `application/pdf`, `text/csv` or the XLSX type.
    pub content_type: String,
    pub size: u64,
    pub sha256: String,
    pub uploaded_by: String,
    pub uploaded_at: jc_time::TimestampTz,
}

impl FileEntry {
    /// The address the file answers under on its portal's hosts.
    pub fn path(&self) -> String {
        format!("/files/{}/{}", self.sha256, url_segment(&self.name))
    }

    /// Raster images are shown inline; every other type downloads.
    pub fn inline(&self) -> bool {
        matches!(
            self.content_type.as_str(),
            "image/png" | "image/jpeg" | "image/webp"
        )
    }
}

const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// The type of `bytes`, read from the bytes alone: PNG, JPEG, WebP, PDF, XLSX, a checked SVG or
/// a CSV (UTF-8 text with no control character but tab and line ends). Anything else is refused.
pub fn sniff(bytes: &[u8]) -> Result<&'static str> {
    let refuse = |why: &str| Err(DataroomError::field_error("file", why));
    if bytes.is_empty() {
        return refuse("the file is empty");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok("image/jpeg");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok("image/webp");
    }
    if bytes.starts_with(b"%PDF-") {
        return Ok("application/pdf");
    }
    if bytes.starts_with(b"PK\x03\x04") {
        // An OOXML workbook names its parts in the zip's local headers in clear text.
        let holds = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        if holds(b"[Content_Types].xml") && holds(b"xl/") {
            return Ok(XLSX);
        }
        return refuse("a zip archive that is no XLSX workbook is not accepted");
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return refuse("not one of PNG, JPEG, WebP, SVG, PDF, CSV or XLSX");
    };
    let head = text.trim_start_matches('\u{feff}').trim_start();
    let lower = head
        .get(..head.len().min(512))
        .unwrap_or("")
        .to_ascii_lowercase();
    if lower.starts_with("<svg") || (lower.starts_with("<?xml") && lower.contains("<svg")) {
        if !crate::branding::is_safe_logo_svg(text) {
            return refuse(
                "the SVG holds a script, an event handler, an external reference or an entity",
            );
        }
        return Ok("image/svg+xml");
    }
    if head.starts_with('<') {
        return refuse("HTML and XML documents are not accepted");
    }
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        return refuse("not one of PNG, JPEG, WebP, SVG, PDF, CSV or XLSX");
    }
    Ok("text/csv")
}

/// A display name: one line of plain text, no path separator, at most [`MAX_NAME_CHARS`].
pub fn check_name(name: &str) -> Result<String> {
    let name = name.trim();
    let bad = |why: &str| Err(DataroomError::field_error("name", why));
    if name.is_empty() {
        return bad("name the file, e.g. hero.png");
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return bad("at most 120 characters");
    }
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        || name == "."
        || name == ".."
    {
        return bad("one line without / or \\, e.g. hero.png");
    }
    Ok(name.to_owned())
}

fn url_segment(name: &str) -> String {
    let mut out = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `12.4 MB` (MiB, as the Files screen writes sizes).
fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The files of every catalogue, on disk under `root`. One process writes it; the lock keeps
/// two writes of one index apart.
#[derive(Debug)]
pub struct FileStore {
    root: PathBuf,
    lock: Mutex<()>,
}

impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lock: Mutex::new(()),
        }
    }

    fn org_dir(&self, org: OrgId) -> PathBuf {
        self.root.join(org.to_string())
    }

    fn dir(&self, org: OrgId, cat: CatalogueId) -> PathBuf {
        self.org_dir(org).join(cat.to_string())
    }

    fn io(e: std::io::Error) -> DataroomError {
        DataroomError::Storage(format!("file store: {e}"))
    }

    fn read_index(dir: &Path) -> Result<Vec<FileEntry>> {
        match fs::read(dir.join("index.json")) {
            Ok(raw) => serde_json::from_slice(&raw)
                .map_err(|e| DataroomError::Storage(format!("file index unreadable: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(Self::io(e)),
        }
    }

    fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir).map_err(Self::io)?;
        let tmp = dir.join(format!(".tmp-{}", uuid::Uuid::now_v7()));
        let mut f = fs::File::create(&tmp).map_err(Self::io)?;
        f.write_all(bytes).map_err(Self::io)?;
        f.sync_all().map_err(Self::io)?;
        fs::rename(&tmp, path).map_err(Self::io)
    }

    fn write_index(dir: &Path, entries: &[FileEntry]) -> Result<()> {
        let raw = serde_json::to_vec_pretty(entries)
            .map_err(|e| DataroomError::Storage(format!("file index: {e}")))?;
        Self::write_atomic(&dir.join("index.json"), &raw)
    }

    /// The files of catalogue `cat`, by name.
    pub fn list(&self, org: OrgId, cat: CatalogueId) -> Result<Vec<FileEntry>> {
        let mut all = Self::read_index(&self.dir(org, cat))?;
        all.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        Ok(all)
    }

    /// What Organization `org` holds over all its catalogues, and its caps.
    pub fn usage(&self, org: OrgId) -> Result<FileUsage> {
        let mut usage = FileUsage {
            files: 0,
            bytes: 0,
            max_files: MAX_ORG_FILES,
            max_bytes: MAX_ORG_BYTES,
            max_file_bytes: MAX_FILE_BYTES,
        };
        let dirs = match fs::read_dir(self.org_dir(org)) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(usage),
            Err(e) => return Err(Self::io(e)),
        };
        for dir in dirs {
            let dir = dir.map_err(Self::io)?;
            for entry in Self::read_index(&dir.path())? {
                usage.files += 1;
                usage.bytes += entry.size;
            }
        }
        Ok(usage)
    }

    /// Refuses `bytes` past a cap, naming the limit; `replacing` frees its own size first.
    fn within_caps(&self, org: OrgId, bytes: u64, replacing: Option<u64>) -> Result<()> {
        if bytes > MAX_FILE_BYTES {
            return Err(DataroomError::field_error(
                "file",
                format!(
                    "the file is {}; one file may hold at most {}",
                    mb(bytes),
                    mb(MAX_FILE_BYTES)
                ),
            ));
        }
        let usage = self.usage(org)?;
        if replacing.is_none() && usage.files >= MAX_ORG_FILES {
            return Err(DataroomError::field_error("file", format!(
                "the Organization holds {MAX_ORG_FILES} files, its limit; delete one it no longer uses"
            )));
        }
        let after = usage.bytes - replacing.unwrap_or(0).min(usage.bytes) + bytes;
        if after > MAX_ORG_BYTES {
            return Err(DataroomError::field_error("file", format!(
                "with it the Organization's files would hold {}; its limit is {}: delete files no portal uses",
                mb(after),
                mb(MAX_ORG_BYTES)
            )));
        }
        Ok(())
    }

    /// Stores `bytes` as a new file of catalogue `cat`.
    pub fn put(
        &self,
        (org, cat): (OrgId, CatalogueId),
        name: &str,
        bytes: &[u8],
        by: &str,
    ) -> Result<FileEntry> {
        let name = check_name(name)?;
        let content_type = sniff(bytes)?;
        let _held = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.within_caps(org, bytes.len() as u64, None)?;
        let dir = self.dir(org, cat);
        let sha256 = hex_sha256(bytes);
        Self::write_atomic(&dir.join(&sha256), bytes)?;
        let entry = FileEntry {
            id: format!("f-{}", uuid::Uuid::now_v7()),
            name,
            content_type: content_type.to_owned(),
            size: bytes.len() as u64,
            sha256,
            uploaded_by: by.to_owned(),
            uploaded_at: jc_time::TimestampTz::now(),
        };
        let mut all = Self::read_index(&dir)?;
        all.push(entry.clone());
        Self::write_index(&dir, &all)?;
        Ok(entry)
    }

    fn change(
        &self,
        (org, cat): (OrgId, CatalogueId),
        id: &str,
        edit: impl FnOnce(&mut FileEntry, &Path) -> Result<()>,
    ) -> Result<(FileEntry, FileEntry)> {
        let _held = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.dir(org, cat);
        let mut all = Self::read_index(&dir)?;
        let entry = all
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| DataroomError::NotFound(format!("no file {id}")))?;
        let before = entry.clone();
        edit(entry, &dir)?;
        let after = entry.clone();
        Self::write_index(&dir, &all)?;
        self.drop_unreferenced(&dir, &all, &before.sha256)?;
        Ok((before, after))
    }

    /// Renames file `id`; its bytes and hash stay, so its address changes only in the name.
    pub fn rename(
        &self,
        key: (OrgId, CatalogueId),
        id: &str,
        name: &str,
    ) -> Result<(FileEntry, FileEntry)> {
        let name = check_name(name)?;
        self.change(key, id, |e, _| {
            e.name = name;
            Ok(())
        })
    }

    /// Replaces the bytes of file `id`: a new hash, so a new immutable address.
    pub fn replace(
        &self,
        key: (OrgId, CatalogueId),
        id: &str,
        bytes: &[u8],
        by: &str,
    ) -> Result<(FileEntry, FileEntry)> {
        let content_type = sniff(bytes)?;
        let size = self
            .list(key.0, key.1)?
            .into_iter()
            .find(|e| e.id == id)
            .map(|e| e.size)
            .ok_or_else(|| DataroomError::NotFound(format!("no file {id}")))?;
        // The caps read every index: checked before the index lock is taken.
        {
            let _held = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            self.within_caps(key.0, bytes.len() as u64, Some(size))?;
        }
        self.change(key, id, |e, dir| {
            let sha256 = hex_sha256(bytes);
            Self::write_atomic(&dir.join(&sha256), bytes)?;
            e.sha256 = sha256;
            e.size = bytes.len() as u64;
            e.content_type = content_type.to_owned();
            e.uploaded_by = by.to_owned();
            e.uploaded_at = jc_time::TimestampTz::now();
            Ok(())
        })
    }

    /// Deletes file `id` and its bytes when no other entry holds the same bytes.
    pub fn delete(&self, (org, cat): (OrgId, CatalogueId), id: &str) -> Result<FileEntry> {
        let _held = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.dir(org, cat);
        let mut all = Self::read_index(&dir)?;
        let at = all
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| DataroomError::NotFound(format!("no file {id}")))?;
        let gone = all.remove(at);
        Self::write_index(&dir, &all)?;
        self.drop_unreferenced(&dir, &all, &gone.sha256)?;
        Ok(gone)
    }

    fn drop_unreferenced(&self, dir: &Path, all: &[FileEntry], sha256: &str) -> Result<()> {
        if all.iter().any(|e| e.sha256 == sha256) || !is_sha256(sha256) {
            return Ok(());
        }
        match fs::remove_file(dir.join(sha256)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Self::io(e)),
            _ => Ok(()),
        }
    }

    /// The file of catalogue `cat` whose bytes hash to `sha256`, with its bytes.
    pub fn read(
        &self,
        (org, cat): (OrgId, CatalogueId),
        sha256: &str,
    ) -> Result<Option<(FileEntry, Vec<u8>)>> {
        if !is_sha256(sha256) {
            return Ok(None);
        }
        let dir = self.dir(org, cat);
        let Some(entry) = Self::read_index(&dir)?
            .into_iter()
            .find(|e| e.sha256 == sha256)
        else {
            return Ok(None);
        };
        match fs::read(dir.join(sha256)) {
            Ok(bytes) => Ok(Some((entry, bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Self::io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    fn key(n: u128) -> (OrgId, CatalogueId) {
        let id = |prefix: u128| format!("00000000-0000-7000-8000-{:012x}", prefix + n);
        (
            OrgId::parse_verified(&id(0xA0)).expect("org"),
            CatalogueId::parse_verified(&id(0xC0)).expect("catalogue"),
        )
    }

    fn store() -> (FileStore, PathBuf) {
        let root = std::env::temp_dir().join(format!("jc-files-{}", uuid::Uuid::now_v7()));
        (FileStore::new(&root), root)
    }

    #[test]
    fn the_type_comes_from_the_bytes_never_the_name() {
        assert_eq!(sniff(PNG).unwrap(), "image/png");
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0JFIF").unwrap(), "image/jpeg");
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 ").unwrap(), "image/webp");
        assert_eq!(sniff(b"%PDF-1.7\n").unwrap(), "application/pdf");
        assert_eq!(
            sniff(b"PK\x03\x04..[Content_Types].xml..xl/workbook.xml").unwrap(),
            XLSX
        );
        assert_eq!(
            sniff("názov;počet\nKošice;3\n".as_bytes()).unwrap(),
            "text/csv"
        );
        assert_eq!(
            sniff(br#"<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>"#).unwrap(),
            "image/svg+xml"
        );
        for refused in [
            &b""[..],
            b"<html><script>alert(1)</script></html>",
            b"PK\x03\x04word/document.xml",
            b"MZ\x90\0\x03\0\0\0",
            b"\x7fELF\x02\x01",
            br#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg"><script>x</script></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="https://evil.example/x.png"/></svg>"#,
        ] {
            assert!(sniff(refused).is_err(), "{}", String::from_utf8_lossy(refused));
        }
    }

    #[test]
    fn names_are_one_line_without_a_path() {
        assert_eq!(check_name("  hero.png ").unwrap(), "hero.png");
        for bad in ["", "../etc/passwd", "a\\b", "a\nb", "..", &"x".repeat(121)] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn put_rename_replace_delete_keep_one_catalogue_apart_from_another() {
        let (files, root) = store();
        let (a, b) = (key(1), key(2));
        let hero = files.put(a, "hero.png", PNG, "ana").unwrap();
        assert_eq!(hero.path(), format!("/files/{}/hero.png", hero.sha256));
        assert!(files.read(a, &hero.sha256).unwrap().is_some());
        assert!(
            files.read(b, &hero.sha256).unwrap().is_none(),
            "B never reads A's file"
        );
        assert!(files.list(b.0, b.1).unwrap().is_empty());

        let (_, renamed) = files.rename(a, &hero.id, "Titulná fotka.png").unwrap();
        assert_eq!(renamed.sha256, hero.sha256);
        assert!(renamed.path().ends_with("/Tituln%C3%A1%20fotka.png"));

        let csv = b"a,b\n1,2\n";
        let (before, after) = files.replace(a, &hero.id, csv, "bo").unwrap();
        assert_eq!(
            (after.id.as_str(), after.content_type.as_str()),
            (hero.id.as_str(), "text/csv")
        );
        assert!(
            files.read(a, &before.sha256).unwrap().is_none(),
            "the old address is gone"
        );
        assert!(
            !files.dir(a.0, a.1).join(&before.sha256).exists(),
            "old bytes removed"
        );

        files.delete(a, &hero.id).unwrap();
        assert!(files.list(a.0, a.1).unwrap().is_empty());
        assert!(!files.dir(a.0, a.1).join(&after.sha256).exists());
        assert!(matches!(
            files.delete(a, &hero.id),
            Err(DataroomError::NotFound(_))
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn caps_refuse_naming_the_limit() {
        let (files, root) = store();
        let big = vec![b'a'; (MAX_FILE_BYTES + 1) as usize];
        let err = files.put(key(3), "big.csv", &big, "ana").unwrap_err();
        assert!(
            err.to_string()
                .contains("one file may hold at most 10.0 MB"),
            "{err}"
        );
        assert_eq!(files.usage(key(3).0).unwrap().files, 0, "nothing stored");
        let _ = fs::remove_dir_all(root);
    }
}
