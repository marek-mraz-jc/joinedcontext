#![forbid(unsafe_code)]
//! Catalogue branding: overrides, titles, logo names, and the logo upload check on real images
//! (fixtures made with ImageMagick: lossy VP8, lossless VP8L, VP8X with alpha, PNG).

use dataroom_core::branding::is_host_name;
use dataroom_core::{
    is_safe_logo_filename, localized_text, validate_and_sanitise_logo, BrandingBlock, DataroomError,
};
use serde_json::json;
use std::collections::BTreeMap;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/logos/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn logo_error(bytes: &[u8], ct: &str) -> String {
    match validate_and_sanitise_logo(bytes, ct).unwrap_err() {
        DataroomError::Validation { fields, .. } => fields["logo"].join(" "),
        other => panic!("{other:?}"),
    }
}

#[test]
fn real_png_and_webp_logos_pass_and_get_a_content_hashed_path() {
    for (file, ct, ext) in [
        ("logo.png", "image/png", "png"),
        ("logo-lossy.webp", "image/webp", "webp"),
        ("logo-lossless.webp", "image/webp", "webp"),
        ("logo-alpha.webp", "image/webp; charset=binary", "webp"),
        // Sniffed when the declared type is generic.
        ("logo-lossy.webp", "application/octet-stream", "webp"),
        ("logo.png", "", "png"),
    ] {
        let bytes = fixture(file);
        let asset =
            validate_and_sanitise_logo(&bytes, ct).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(asset.extension, ext, "{file}");
        assert_eq!(asset.content_type, format!("image/{ext}"));
        assert_eq!(
            asset.url_path,
            format!("/assets/logo-{}.{ext}", &asset.sha256_hex[..16])
        );
        assert_eq!(asset.bytes, bytes);
    }
}

#[test]
fn oversized_or_broken_rasters_are_refused() {
    assert!(logo_error(&fixture("logo-too-wide.png"), "image/png")
        .contains("PNG dimensions 5000x2 exceed"));
    assert!(logo_error(&fixture("logo-too-wide.webp"), "image/webp")
        .contains("WebP dimensions 5000x8 exceed"));
    assert_eq!(
        logo_error(b"\x89PNG\r\n\x1a\nshort", "image/png"),
        "invalid PNG header"
    );
    assert_eq!(logo_error(&[0u8; 40], "image/webp"), "invalid WebP header");
    assert_eq!(
        logo_error(b"GIF89a.....", "image/gif"),
        "unsupported image format; only SVG, PNG and WebP are allowed"
    );
    assert_eq!(logo_error(b"", "image/png"), "empty image upload");
    assert!(logo_error(&vec![0u8; 1024 * 1024 + 1], "image/png").contains("exceeds limit"));

    // A RIFF/WEBP file whose first chunk is not an image chunk, or whose signatures are wrong.
    let mut odd = fixture("logo-lossy.webp");
    odd[12..16].copy_from_slice(b"ALPH");
    assert_eq!(logo_error(&odd, "image/webp"), "invalid WebP header");
    let mut bad_start = fixture("logo-lossy.webp");
    bad_start[23] = 0;
    assert_eq!(logo_error(&bad_start, "image/webp"), "invalid WebP header");
    let mut bad_sig = fixture("logo-lossless.webp");
    bad_sig[20] = 0;
    assert_eq!(logo_error(&bad_sig, "image/webp"), "invalid WebP header");
    // A zero-sized lossy frame.
    let mut zero = fixture("logo-lossy.webp");
    zero[26] = 0;
    zero[27] = 0;
    assert!(logo_error(&zero, "image/webp").contains("WebP dimensions 0x40"));
    // A PNG header declaring zero width.
    let mut png = fixture("logo.png");
    png[16..20].copy_from_slice(&[0, 0, 0, 0]);
    assert!(logo_error(&png, "image/png").contains("PNG dimensions 0x40"));
    // Non-UTF-8 bytes declared as SVG.
    assert_eq!(
        logo_error(&[0xff, 0xfe, 0x00], "image/svg+xml"),
        "SVG must be valid UTF-8"
    );
}

#[test]
fn flat_colour_overrides_take_only_hex_colours() {
    let b = BrandingBlock::from_json_lossy(&json!({
        "primary": "#112233", "secondary": "#445566", "accent": "#778899",
        "background": "#ffffff", "text": "#000000"
    }));
    assert_eq!(
        [
            b.colours.primary.as_str(),
            b.colours.secondary.as_str(),
            b.colours.accent.as_str(),
            b.colours.background.as_str(),
            b.colours.text.as_str()
        ],
        ["#112233", "#445566", "#778899", "#ffffff", "#000000"]
    );
    assert_eq!(b.primary_rgb, "17, 34, 51");
    let neutral = BrandingBlock::default();
    let bad = BrandingBlock::from_json_lossy(&json!({
        "primary": "red", "secondary": "#12", "accent": "url(x)", "background": "#gggggg", "text": 5
    }));
    assert_eq!(bad.colours, neutral.colours);
}

#[test]
fn site_title_falls_back_to_the_instance_name_then_the_product() {
    let mut b = BrandingBlock::from_json_lossy(&json!({
        "siteTitle": {"sk": "Otvorené dáta", "en": ""}, "instanceName": "Mesto"
    }));
    assert_eq!(b.site_title_in("sk"), "Otvorené dáta");
    assert_eq!(b.site_title_in("en"), "Otvorené dáta");
    b.site_title = BTreeMap::new();
    assert_eq!(b.site_title_in("en"), "Mesto");
    b.instance_name.clear();
    assert_eq!(b.site_title_in("en"), "joinedcontext");
    let one = BrandingBlock::from_json_lossy(&json!({"site_title": "Dáta mesta"}));
    assert_eq!(one.site_title_in("cs"), "Dáta mesta");
}

#[test]
fn logo_names_and_paths_stay_on_this_origin() {
    for good in ["logo.svg", "Mesto_2-x.png", "a.webp", "1.b.svg"] {
        assert!(is_safe_logo_filename(good), "{good}");
    }
    for bad in [
        "logo",
        "logo.gif",
        ".svg",
        "-a.svg",
        "../a.svg",
        "a/b.svg",
        "a b.png",
        "https://x/a.svg",
        "//x/a.svg",
    ] {
        assert!(!is_safe_logo_filename(bad), "{bad}");
    }
    let b = BrandingBlock::from_json_lossy(&json!({"siteLogo": "mesto.svg"}));
    assert_eq!(b.site_logo_path("/catalogue/"), "/catalogue/mesto.svg");
    for refused in [
        "https://evil.example/logo.svg",
        "//evil.example/logo.svg",
        "javascript:alert(1)",
    ] {
        let b = BrandingBlock::from_json_lossy(&json!({ "siteLogo": refused }));
        assert_eq!(b.site_logo, None, "{refused}");
        assert_eq!(b.site_logo_path(""), "/jc-logo.svg");
    }
}

#[test]
fn localized_text_prefers_the_language_then_the_catalogue_default() {
    let v: BTreeMap<String, String> = [("sk", "Ahoj"), ("en", "Hello"), ("cs", "")]
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
    assert_eq!(localized_text(&v, "en", "sk"), Some("Hello"));
    assert_eq!(localized_text(&v, "cs", "sk"), Some("Ahoj"));
    assert_eq!(localized_text(&v, "de", "pl"), Some("Hello"));
    assert_eq!(localized_text(&BTreeMap::new(), "de", "pl"), None);
    assert!(is_host_name("data.mesto.sk"));
    assert!(!is_host_name("localhost") && !is_host_name("-a.sk") && !is_host_name("A.sk"));
}

#[test]
fn footer_lines_and_taglines_drop_empty_entries() {
    let b = BrandingBlock::from_json_lossy(&json!({
        "footerLines": ["  Mesto Banská Bystrica ", "   ", 5, "Námestie SNP 1"],
        "siteTagline": {"sk": "Dáta mesta", "en": ""}
    }));
    assert_eq!(
        b.footer_lines,
        vec!["Mesto Banská Bystrica", "Námestie SNP 1"]
    );
    assert_eq!(b.site_tagline.len(), 1);
    assert_eq!(b.site_tagline_in("en").as_deref(), Some("Dáta mesta"));
    let plain = BrandingBlock::from_json_lossy(
        &json!({"site_tagline": "Otvorené dáta", "footer_lines": "not a list"}),
    );
    assert_eq!(
        plain.site_tagline_in("cs").as_deref(),
        Some("Otvorené dáta")
    );
    assert_eq!(plain.footer_lines, BrandingBlock::default().footer_lines);
}
