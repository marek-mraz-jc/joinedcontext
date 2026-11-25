//! The OCI layout a build pod uploads (AP-130): any bytes are refused with a reason or read as
//! an image whose manifest is followed by a config and at least one layer, never a panic.
//! The first line is the digest the build recorded, the rest the layout.
#![no_main]

use joinedcontext_portal::apps::built::image_of;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let split = data.iter().position(|b| *b == b'\n').unwrap_or(0);
    let (digest, layout) = data.split_at(split);
    let Ok(digest) = std::str::from_utf8(digest) else {
        return;
    };
    let layout = layout.get(1..).unwrap_or_default();
    if let Ok(image) = image_of(layout, digest) {
        assert!(image.blobs.len() >= 2, "a config and a layer");
        assert!(!image.manifest.is_empty());
    }
});
