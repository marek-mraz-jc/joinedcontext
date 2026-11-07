//! The static host's copy of a published build (AP-102, ADR-N-028).
//!
//! `status.build` names a build by commit and digest; the package `app-{name}`, version
//! `{commit}`, holds it as `bundle.tar.gz` (AP-101). Every replica fetches the builds its own
//! mirror names, because each one serves `/apps/*` from its own disk: it downloads the archive
//! read-only, checks its SHA-256 against the digest before a byte is unpacked, and unpacks it
//! into `{cache}/{name}/{hex}/`. A build that cannot be fetched or does not match leaves the
//! previous one serving, and the leader's `BuildMissing` condition says so (AP-72).

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::apps::built::{package, version, MAX_BUNDLE_BYTES};
use crate::git::{GitError, GiteaClient};
use crate::store::Mirror;

/// The most an unpacked bundle may hold, so a small archive cannot fill the disk.
/// ponytail: fixed bounds for every application; a setting when one needs more.
const MAX_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

/// Why a build was not fetched.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("the forge did not hand over the package: {0}")]
    Forge(#[from] GitError),
    #[error("bundle.tar.gz hashes to {found}, not the digest {expected} status.build names")]
    Mismatch { expected: String, found: String },
    #[error("the bundle cannot be unpacked: {0}")]
    Unpack(String),
}

/// `{cache}/{name}/{hex}/`, the directory one build is served from, or `None` for a name or a
/// digest no build could carry, so neither ever becomes a path of its own.
pub fn build_dir(cache: &Path, name: &str, digest: &str) -> Option<PathBuf> {
    let hex = digest.strip_prefix("sha256:")?;
    let well_formed = hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    (well_formed && crate::resource::is_dns1123(name)).then(|| cache.join(name).join(hex))
}

/// Fetches every build the mirror's Apps name and this replica does not hold yet. One App that
/// fails is logged and retried on the next sync; it never stops the others.
pub async fn fetch_missing(gitea: &GiteaClient, cache: &Path, mirror: &Mirror) {
    for envelope in mirror.matching(|env| env.kind == "App") {
        let Some(build) = envelope.status.as_ref().and_then(|s| s.build.as_ref()) else {
            continue;
        };
        let name = envelope.metadata.name.as_str();
        let Some(dir) = build_dir(cache, name, &build.digest) else {
            continue;
        };
        if dir.is_dir() {
            continue;
        }
        match fetch(gitea, &dir, name, &build.commit, &build.digest).await {
            Ok(()) => tracing::info!(app = %name, digest = %build.digest, "build fetched"),
            Err(err) => tracing::warn!(app = %name, error = %err, "build not fetched"),
        }
    }
}

async fn fetch(
    gitea: &GiteaClient,
    dir: &Path,
    name: &str,
    commit: &str,
    digest: &str,
) -> Result<(), FetchError> {
    // The applications' organization publishes every build (AP-101). A build published before
    // its repository moved there (T-2969) stays with the organization that owned the repository
    // then, the configuration's, since the registry has no transfer: the same package and
    // version is read there, and the digest check below still decides, so a move never
    // unserves an App and nothing but the build `status.build` names is served (T-3026).
    let applications = gitea.applications();
    let bytes = match read_bundle(&applications, name, commit, digest).await {
        Err(GitError::NotFound) if applications.owner != gitea.owner => {
            read_bundle(gitea, name, commit, digest).await?
        }
        other => other?,
    };
    let found = format!("sha256:{:x}", Sha256::digest(&bytes));
    if found != digest {
        return Err(FetchError::Mismatch {
            expected: digest.to_owned(),
            found,
        });
    }
    let dir = dir.to_owned();
    tokio::task::spawn_blocking(move || install(&bytes, &dir))
        .await
        .map_err(|err| FetchError::Unpack(err.to_string()))?
}

/// `bundle.tar.gz` of the build of `commit` with `digest`, from `owner`'s registry. A build
/// published before T-2671 sits under the bare commit.
async fn read_bundle(
    owner: &GiteaClient,
    name: &str,
    commit: &str,
    digest: &str,
) -> Result<Vec<u8>, GitError> {
    let package = package(name);
    let read = |version: String| {
        let package = package.clone();
        async move {
            owner
                .get_generic_file(&package, &version, "bundle.tar.gz", MAX_BUNDLE_BYTES)
                .await
        }
    };
    match read(version(commit, digest)).await {
        Err(GitError::NotFound) => read(commit.to_owned()).await,
        other => other,
    }
}

/// Unpacks the archive beside `dir` and renames it into place, so a reader sees the whole build
/// or none of it.
fn install(archive: &[u8], dir: &Path) -> Result<(), FetchError> {
    let (Some(parent), Some(leaf)) = (dir.parent(), dir.file_name()) else {
        return Err(FetchError::Unpack(format!(
            "{} has no parent",
            dir.display()
        )));
    };
    let partial = parent.join(format!(".partial-{}", leaf.to_string_lossy()));
    let unpack_error = |err: io::Error| FetchError::Unpack(err.to_string());
    if partial.exists() {
        fs::remove_dir_all(&partial).map_err(unpack_error)?;
    }
    fs::create_dir_all(&partial).map_err(unpack_error)?;
    let unpacked = unpack(archive, &partial).and_then(|()| {
        if partial.join("index.html").is_file() {
            Ok(())
        } else {
            Err(FetchError::Unpack(
                "the bundle has no index.html".to_owned(),
            ))
        }
    });
    if let Err(err) = unpacked {
        let _ = fs::remove_dir_all(&partial);
        return Err(err);
    }
    match fs::rename(&partial, dir) {
        Ok(()) => {}
        // Another sync of this replica got there first: the same digest is the same build.
        Err(_) if dir.is_dir() => {
            let _ = fs::remove_dir_all(&partial);
        }
        Err(err) => return Err(unpack_error(err)),
    }
    prune(parent, leaf);
    Ok(())
}

/// Removes the app's other builds once `keep` is in place: the cache holds one build per app,
/// so it never grows past the catalog (the pod is evicted when its volume is full).
fn prune(app: &Path, keep: &std::ffi::OsStr) {
    let Ok(entries) = fs::read_dir(app) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() != keep {
            if let Err(err) = fs::remove_dir_all(entry.path()) {
                tracing::warn!(path = %entry.path().display(), error = %err, "old build kept");
            }
        }
    }
}

/// Unpacks a `.tar.gz` into `dest`: regular files and directories only, every path inside
/// `dest`, no file written twice, and at most `MAX_UNPACKED_BYTES` in `MAX_ENTRIES` entries.
/// A link, a device or a path that climbs out refuses the whole archive.
pub fn unpack(archive: &[u8], dest: &Path) -> Result<(), FetchError> {
    let refuse = |why: String| FetchError::Unpack(why);
    let io_error = |err: io::Error| FetchError::Unpack(err.to_string());
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut budget = MAX_UNPACKED_BYTES;
    for (index, entry) in tar.entries().map_err(io_error)?.enumerate() {
        if index >= MAX_ENTRIES {
            return Err(refuse(format!("more than {MAX_ENTRIES} entries")));
        }
        let mut entry = entry.map_err(io_error)?;
        let raw = entry.path().map_err(io_error)?.into_owned();
        let mut relative = PathBuf::new();
        for component in raw.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(part) => relative.push(part),
                _ => return Err(refuse(format!("{} leaves the bundle", raw.display()))),
            }
        }
        let target = dest.join(&relative);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            fs::create_dir_all(&target).map_err(io_error)?;
        } else if kind.is_file() {
            if relative.as_os_str().is_empty() {
                return Err(refuse("a file with no name".to_owned()));
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(io_error)?;
            }
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|err| refuse(format!("{}: {err}", relative.display())))?;
            let written =
                io::copy(&mut (&mut entry).take(budget + 1), &mut file).map_err(io_error)?;
            if written > budget {
                return Err(refuse(format!(
                    "more than {MAX_UNPACKED_BYTES} bytes unpacked"
                )));
            }
            budget -= written;
        } else {
            return Err(refuse(format!(
                "{} is a {kind:?}, and a bundle holds files and directories only",
                raw.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `.tar.gz` of `(path, kind, bytes)`, written with raw headers so a hostile path is kept
    /// as it is instead of being refused by the builder.
    fn archive(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, kind, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            let name = &mut header.as_old_mut().name;
            name[..path.len()].copy_from_slice(path.as_bytes());
            header.set_entry_type(*kind);
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            if *kind == tar::EntryType::Symlink {
                header.set_link_name("/etc/passwd").expect("link name");
            }
            header.set_cksum();
            builder.append(&header, *bytes).expect("append");
        }
        let tar = builder.into_inner().expect("tar");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        io::Write::write_all(&mut gz, &tar).expect("gzip");
        gz.finish().expect("gzip")
    }

    /// A fresh directory of this test process, removed when it goes out of scope.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("jc-portal-fetch-{test}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("scratch directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const FILE: tar::EntryType = tar::EntryType::Regular;
    const DIR: tar::EntryType = tar::EntryType::Directory;

    /// AP-102: the archive the lane packs (`tar -C bundle -cf - .`) lands as the bundle's tree.
    #[test]
    fn the_lanes_archive_unpacks_as_the_bundles_tree() {
        let dest = Scratch::new("tree");
        let bytes = archive(&[
            ("./", DIR, b""),
            ("./index.html", FILE, b"<html></html>"),
            ("./assets/", DIR, b""),
            ("./assets/app.js", FILE, b"console.log(1)"),
        ]);
        unpack(&bytes, dest.path()).expect("unpacks");
        assert_eq!(
            fs::read(dest.path().join("assets/app.js")).expect("file"),
            b"console.log(1)"
        );
    }

    /// AP-102: a path that climbs out of the bundle refuses the whole archive.
    #[test]
    fn a_path_leaving_the_bundle_is_refused() {
        let dest = Scratch::new("climb");
        for path in [
            "../escape.html",
            "/etc/escape.html",
            "./a/../../escape.html",
        ] {
            let bytes = archive(&[(path, FILE, b"x")]);
            let err = unpack(&bytes, &dest.path().join("app")).expect_err(path);
            assert!(
                err.to_string().contains("leaves the bundle"),
                "{path}: {err}"
            );
        }
        assert!(!dest.path().join("escape.html").exists());
    }

    /// AP-102: a link or a device is refused, so no file of the bundle can point elsewhere.
    #[test]
    fn a_link_in_the_bundle_is_refused() {
        let dest = Scratch::new("link");
        let bytes = archive(&[("index.html", tar::EntryType::Symlink, b"")]);
        let err = unpack(&bytes, dest.path()).expect_err("a symlink");
        assert!(
            err.to_string().contains("files and directories only"),
            "{err}"
        );
    }

    /// AP-102: a path written twice is refused rather than letting the second copy win.
    #[test]
    fn a_file_written_twice_is_refused() {
        let dest = Scratch::new("twice");
        let bytes = archive(&[("index.html", FILE, b"one"), ("./index.html", FILE, b"two")]);
        assert!(unpack(&bytes, dest.path()).is_err());
    }

    /// AP-102: the build is installed whole or not at all, and one with no `index.html` is none.
    #[test]
    fn a_bundle_without_an_index_is_not_installed() {
        let cache = Scratch::new("index");
        let dir = cache.path().join("alerts").join("ab");
        let err = install(&archive(&[("app.js", FILE, b"x")]), &dir).expect_err("no index");
        assert!(err.to_string().contains("no index.html"), "{err}");
        assert!(!dir.exists());
        assert!(!cache.path().join("alerts").join(".partial-ab").exists());

        install(&archive(&[("index.html", FILE, b"<html>")]), &dir).expect("installs");
        assert!(dir.join("index.html").is_file());
        // The same build again is the same build: installing it twice is not an error.
        install(&archive(&[("index.html", FILE, b"<html>")]), &dir).expect("again");
    }

    /// AP-102: the next build replaces the previous one on disk, so the cache holds one per app.
    #[test]
    fn the_next_build_removes_the_previous_one() {
        let cache = Scratch::new("prune");
        let app = cache.path().join("alerts");
        install(&archive(&[("index.html", FILE, b"one")]), &app.join("aa")).expect("first");
        install(&archive(&[("index.html", FILE, b"two")]), &app.join("bb")).expect("second");
        let held: Vec<_> = fs::read_dir(&app)
            .expect("app dir")
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(held, vec![std::ffi::OsString::from("bb")]);
    }

    /// AP-102: only a `sha256:` digest of 64 hex digits and a DNS-1123 name become a directory.
    #[test]
    fn only_a_well_formed_digest_and_name_become_a_directory() {
        let cache = Path::new("/cache");
        let hex = "a".repeat(64);
        assert_eq!(
            build_dir(cache, "alerts", &format!("sha256:{hex}")),
            Some(cache.join("alerts").join(&hex))
        );
        for (name, digest) in [
            ("alerts", hex.clone()),
            ("alerts", format!("sha256:{}", "A".repeat(64))),
            ("alerts", "sha256:../../etc".to_owned()),
            ("../alerts", format!("sha256:{hex}")),
        ] {
            assert_eq!(build_dir(cache, name, &digest), None, "{name} {digest}");
        }
    }
}
