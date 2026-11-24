//! What a Git client sends to upload-pack, read without a login on a public repository. The first
//! byte picks the reader: 0 the pkt-line list, 1 a Protocol v0 negotiation, 2 a Protocol v2
//! command, 3 a gzipped spooled request, 4 a `filter` spec, 5 a blob for the secret scan. Every
//! input must end in an answer or a refusal, never a panic, and a parsed pkt-line list never holds
//! more bytes than its input.
#![no_main]

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use shako_core::{InstanceId, RepoId};
use shako_git::{
    ObjectFilter, PktLine, handle_upload_pack, init_bare_repo, parse_all_pkt_lines,
    read_negotiation, scan_blob_for_secrets,
};

static ROOT: LazyLock<tempfile::TempDir> =
    LazyLock::new(|| tempfile::tempdir().expect("a repository root"));

/// One repository with an initial commit on `main`, never written to by a fetch.
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

/// Holds at most 64 MiB: a pack of the seeded repository is a few hundred bytes.
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
    match mode % 6 {
        0 => {
            if let Ok(lines) = parse_all_pkt_lines(rest) {
                let held: usize = lines
                    .iter()
                    .map(|l| match l {
                        PktLine::Data(d) => d.len(),
                        _ => 0,
                    })
                    .sum();
                assert!(held <= rest.len());
            }
        }
        1 | 2 => {
            let protocol = (mode % 6 == 2).then_some("version=2");
            let mut out = Vec::new();
            let _ = handle_upload_pack(&REPO, protocol, rest, &mut out, &mut admit);
        }
        3 => {
            let mut spool = tempfile::tempfile().expect("spool");
            spool.write_all(rest).expect("spool write");
            let mut budget = admit;
            if let Ok(request) = read_negotiation(&spool, true, 1 << 20, &mut budget) {
                assert!(request.len() as u64 <= (1 << 20) + (1 << 20));
            }
        }
        4 => {
            let _ = ObjectFilter::parse(&String::from_utf8_lossy(rest));
        }
        _ => {
            if let Some(found) = scan_blob_for_secrets("config/app.env", rest) {
                assert!(!found.is_empty());
            }
        }
    }
});
