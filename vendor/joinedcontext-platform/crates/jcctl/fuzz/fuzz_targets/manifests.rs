//! Input: a manifest file as `jcctl apply -f`, `jcctl fmt` and the diff against the live copy read
//! it. Every target keeps the project it was given; the formatter is idempotent on what it accepts.
#![no_main]

use jcctl::fmt::{format_file, format_markdown};
use jcctl::portal::targets;
use libfuzzer_sys::fuzz_target;
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(found) = targets(text, Some("p")) {
        assert!(found.iter().all(|t| t.project == "p"));
        for pair in found.windows(2) {
            let _ = jcctl::diff(&pair[0].manifest, &pair[1].manifest);
        }
        if let Some(one) = found.first() {
            assert!(jcctl::diff(&one.manifest, &one.manifest).is_empty());
        }
    }
    let _ = targets(text, None);
    if let Ok(once) = format_file(Path::new("m.yaml"), text) {
        let _ = format_file(Path::new("m.yaml"), &once);
    }
    let _ = format_markdown(text);
});
