//! Input: one byte that picks the declared content type, then the uploaded logo's bytes. The
//! check must refuse or answer an asset whose bytes, type and hashed path agree; an SVG it
//! accepts must pass the SVG check again.
#![no_main]

use dataroom_core::branding::{is_safe_logo_svg, validate_and_sanitise_logo};
use libfuzzer_sys::fuzz_target;

const TYPES: [&str; 6] = [
    "image/svg+xml",
    "image/png",
    "image/webp",
    "image/jpeg",
    "",
    "image/svg+xml; charset=utf-8",
];

fuzz_target!(|data: &[u8]| {
    let Some((pick, bytes)) = data.split_first() else {
        return;
    };
    let declared = TYPES[usize::from(*pick) % TYPES.len()];
    if let Ok(asset) = validate_and_sanitise_logo(bytes, declared) {
        assert!(asset.bytes.len() <= 1024 * 1024, "asset of {} bytes", asset.bytes.len());
        assert_eq!(asset.sha256_hex.len(), 64);
        assert!(asset.url_path.contains(&asset.sha256_hex[..16]), "{}", asset.url_path);
        assert!(asset.url_path.ends_with(&format!(".{}", asset.extension)));
        if asset.content_type == "image/svg+xml" {
            let text = std::str::from_utf8(&asset.bytes).expect("an accepted SVG is UTF-8");
            assert!(is_safe_logo_svg(text), "accepted SVG fails its own check");
        }
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        let _ = is_safe_logo_svg(text);
    }
});
