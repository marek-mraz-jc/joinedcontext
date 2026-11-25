//! An archive download names its repository and ref in the URL, read without a login on a public
//! repository. The first byte picks zip or tar.gz; the rest is the repository name, a NUL, then
//! the ref. Every input must end in an archive or a refusal (4xx), never a panic or a 5xx: the
//! repository is whole, so only the request can be wrong.
#![no_main]

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use shako_core::{InstanceId, RepoId};
use shako_git::{ArchiveFormat, create_archive, init_bare_repo};

static ROOT: LazyLock<tempfile::TempDir> =
    LazyLock::new(|| tempfile::tempdir().expect("a repository root"));

/// One repository with an initial commit on `main`, only read.
static REPO: LazyLock<PathBuf> = LazyLock::new(|| {
    init_bare_repo(
        ROOT.path(),
        InstanceId::from_u128(1),
        RepoId::from_u128(1),
        "main",
        true,
        "fuzz",
    )
    .expect("a seeded repository")
});

/// Holds at most 64 MiB: the seeded repository's archive is a few hundred bytes.
fn admit(bytes: u64) -> shako_core::ShakoResult<()> {
    if bytes > 64 << 20 {
        return Err(shako_core::ShakoError::Overloaded("fuzz budget".into()));
    }
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let format = if mode % 2 == 0 {
        ArchiveFormat::Zip
    } else {
        ArchiveFormat::TarGz
    };
    let (name, git_ref) = match rest.iter().position(|b| *b == 0) {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => (&b"repo"[..], rest),
    };
    let name = String::from_utf8_lossy(name);
    let git_ref = String::from_utf8_lossy(git_ref);
    let mut out = Cursor::new(Vec::new());
    match create_archive(&REPO, &name, &git_ref, format, &mut out, &mut admit) {
        Ok(()) => assert!(!out.get_ref().is_empty(), "an archive has bytes"),
        Err(e) => assert!(
            (400..500).contains(&e.status_code()),
            "a refusal, not a failure: {e}"
        ),
    }
});
