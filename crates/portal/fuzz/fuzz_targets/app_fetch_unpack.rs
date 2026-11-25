//! An App's published bundle unpacked into the serving directory (AP-12): whatever the tar.gz
//! holds, nothing lands outside the destination and nothing but files and directories lands.
#![no_main]

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use joinedcontext_portal::apps::fetch::unpack;
use libfuzzer_sys::fuzz_target;

static RUN: AtomicU64 = AtomicU64::new(0);

/// Every entry under `dir`, checked to be a plain file or directory inside `root`.
fn inside(root: &Path, dir: &Path) {
    for entry in std::fs::read_dir(dir).expect("a readable destination") {
        let entry = entry.expect("an entry");
        let kind = entry.file_type().expect("a file type");
        assert!(kind.is_file() || kind.is_dir(), "{:?}", entry.path());
        let path = entry
            .path()
            .canonicalize()
            .expect("an unpacked path resolves");
        assert!(
            path.starts_with(root),
            "{} left {}",
            path.display(),
            root.display()
        );
        if kind.is_dir() {
            inside(root, &entry.path());
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let base = std::env::temp_dir().join(format!(
        "jc-fuzz-unpack-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    let dest = base.join("app");
    std::fs::create_dir_all(&dest).expect("a destination");
    let _ = unpack(data, &dest);
    let root = dest.canonicalize().expect("the destination resolves");
    inside(&root, &dest);
    assert_eq!(
        std::fs::read_dir(&base).expect("the base").count(),
        1,
        "nothing beside the destination"
    );
    std::fs::remove_dir_all(&base).expect("clean up");
});
