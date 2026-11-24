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

#[test]
fn a_url_field_is_typed_any_uri_and_three_digit_colours_expand() {
    use dataroom_core::branding::{parse_rgb, BrandingDatasetFields};
    let field: jc_core::kinds::DatasetField =
        serde_json::from_value(json!({"name": "web", "label": {}, "type": "url"})).unwrap();
    assert_eq!(BrandingDatasetFields::xsd_type(&field), Some("anyURI"));
    assert_eq!(parse_rgb("#abc"), Some((0xaa, 0xbb, 0xcc)));
    assert_eq!(parse_rgb("#a1b2c3"), Some((0xa1, 0xb2, 0xc3)));
    for bad in ["red", "#abcd", "abc", "#ggg", ""] {
        assert_eq!(parse_rgb(bad), None, "{bad:?}");
    }
}

#[test]
fn a_branding_string_is_json_first_then_the_yaml_block() {
    let json = BrandingBlock::from_str_lossy(r#"{"instanceName": "Mesto Dáta", "noindex": true}"#);
    assert_eq!(json.instance_name, "Mesto Dáta");
    assert!(json.noindex);
    let yaml = BrandingBlock::from_str_lossy(
        "branding:\n  instanceName: 'Mesto'\n  noindex: true\n  domain: '{{ env \"DOMAIN\" | default \"city.example\" }}'\n  documentationBaseUrl: 'https://{{ .Values.host | default \"docs.city.example\" }}/guide'\n  city: '{{ default \"unterminated }}'\n  extra:\n    nested: value\n  languages:\n    - sk\n    - ''\n    - en\n    default: sk\n    note: ignored\n",
    );
    assert_eq!(yaml.instance_name, "Mesto");
    assert!(yaml.noindex);
    assert_eq!(yaml.domain, "city.example", "a Helm default resolves");
    assert_eq!(
        yaml.documentation_base_url,
        "https://docs.city.example/guide"
    );
    assert_eq!(
        yaml.languages.offered,
        ["sk", "en"],
        "an empty code is skipped"
    );
    assert_eq!(yaml.languages.default, "sk");
    assert_eq!(
        yaml.city, "{{ default \"unterminated }}",
        "a default without its closing quote stays as written"
    );
}

#[test]
fn an_override_keeps_the_level_above_wherever_a_value_is_unsafe_or_empty() {
    let base = BrandingBlock::default();
    let mut b = base.clone();
    b.apply_override_json(&json!({
        "colours": {"primary": "red", "secondary": "#12", "accent": 7, "background": "url(x)",
                    "text": "#gggggg"},
        "fonts": {"heading": "x; } body { color: red", "body": "url(https://evil.example/f)"},
        "languages": {"default": "", "offered": ["", "  "]},
        "tagline": 5, "ribbon": ["x"], "siteTitle": 3, "siteTagline": false,
        "siteLogo": "../etc/passwd.svg", "samples": {"sk": ["", "  "], "en": "not a list"}
    }));
    assert_eq!(b.colours, base.colours);
    assert_eq!(b.fonts, base.fonts);
    assert_eq!(b.languages, base.languages);
    assert_eq!(
        (b.tagline.as_str(), b.ribbon.as_str()),
        (base.tagline.as_str(), base.ribbon.as_str())
    );
    assert_eq!(
        (&b.site_title, &b.site_tagline, &b.site_logo),
        (&base.site_title, &base.site_tagline, &base.site_logo)
    );
    assert!(b.samples.is_empty(), "{:?}", b.samples);

    b.apply_override_json(
        &json!({"siteIssued": "2026-01-15", "siteTitle": {"sk": "", "en": "City"}}),
    );
    assert_eq!(b.site_issued.as_deref(), Some("2026-01-15"));
    assert_eq!(b.site_title.get("en").map(String::as_str), Some("City"));
    assert!(
        !b.site_title.contains_key("sk"),
        "an empty title is not taken"
    );
}

#[test]
fn the_header_tagline_and_logo_fall_back_in_order() {
    let mut b = BrandingBlock::default();
    b.site_tagline.clear();
    b.tagline = "Otvorené dáta".into();
    assert_eq!(b.site_tagline_in("sk").as_deref(), Some("Otvorené dáta"));
    b.tagline.clear();
    assert_eq!(b.site_tagline_in("sk"), None);

    b.site_logo = Some("/assets/logo.svg".into());
    assert_eq!(b.site_logo_path("/portal/"), "/portal/assets/logo.svg");
    b.site_logo = Some("logo-city.png".into());
    assert_eq!(b.site_logo_path("/portal"), "/portal/logo-city.png");
    b.site_logo = Some("bad name.exe".into());
    assert_eq!(
        b.site_logo_path(""),
        "/jc-logo.svg",
        "an unsafe name is never linked"
    );
}

#[test]
fn a_publisher_image_prefers_its_logo_then_the_fallback_logo_then_a_badge() {
    use dataroom_core::branding::{resolve_publisher_branding, PublisherBranding};
    let none = BTreeMap::new();
    let fb = PublisherBranding {
        initials: Some("BB".into()),
        colour: Some("#112233".into()),
        logo_url: Some("https://city.example/logo.png".into()),
        ..PublisherBranding::default()
    };
    let (img, _) = resolve_publisher_branding(
        Some("https://own.example/l.svg"),
        &none,
        Some(&fb),
        "sk",
        "sk",
    );
    assert_eq!(img.as_deref(), Some("https://own.example/l.svg"));
    let (img, _) = resolve_publisher_branding(Some(""), &none, Some(&fb), "sk", "sk");
    assert_eq!(img.as_deref(), Some("https://city.example/logo.png"));
    let badge = PublisherBranding {
        logo_url: Some(String::new()),
        ..fb.clone()
    };
    let (img, _) = resolve_publisher_branding(None, &none, Some(&badge), "sk", "sk");
    assert!(img.unwrap().starts_with("data:image/svg+xml;base64,"));
    let bare = PublisherBranding {
        logo_url: None,
        colour: None,
        ..fb
    };
    assert_eq!(
        resolve_publisher_branding(None, &none, Some(&bare), "sk", "sk").0,
        None
    );
}

#[test]
fn svg_checks_skip_comments_and_declarations_but_never_an_attribute() {
    let is_safe_logo_svg =
        |svg: &str| validate_and_sanitise_logo(svg.as_bytes(), "image/svg+xml").is_ok();
    let err = logo_error(b"<svg-ish>no root</svg-ish>", "image/svg+xml");
    assert!(err.contains("root <svg>"), "{err}");
    let ok = [
        "<?xml version=\"1.0\"?><!DOCTYPE svg><svg xmlns=\"http://www.w3.org/2000/svg\"><!-- a comment onload=x --></svg>",
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M0 0\"/></svg><!-- left open",
        "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg><?xml left open",
    ];
    for svg in ok {
        assert!(is_safe_logo_svg(svg), "{svg}");
    }
    // a stray `=` hides nothing: an HTML parser reads `onload` after it as an attribute
    assert!(!is_safe_logo_svg("<svg = onload=\"alert(1)\"></svg>"));
    let err = logo_error(b"<svg = width=\"10\"></svg>", "image/svg+xml");
    assert!(err.contains("an attribute without a name"), "{err}");
}

#[test]
fn yaml_colours_fonts_and_a_logo_svg_reach_the_block() {
    use dataroom_core::branding::{sanitize_font_stack, BrandingDatasetFields};
    let field: jc_core::kinds::DatasetField =
        serde_json::from_value(json!({"name": "budget", "label": {}, "type": "number"})).unwrap();
    assert_eq!(BrandingDatasetFields::xsd_type(&field), Some("decimal"));
    assert_eq!(sanitize_font_stack("   "), None);
    let b = BrandingBlock::from_yaml_lossy(
        "branding:\n  domain: 'https://{{ default \"docs.example\" '\n  tagline: 'Dáta mesta'\n  colours:\n    primary: '#112233'\n  fonts:\n    heading: 'Inter, sans-serif'\n",
    );
    assert_eq!(b.colours.primary, "#112233");
    assert_eq!(b.fonts.heading, "Inter, sans-serif");
    assert_eq!(b.tagline, "Dáta mesta");
    assert_eq!(
        b.domain,
        BrandingBlock::default().domain,
        "a Helm expression without its closing braces is no host"
    );
    let mut svg = BrandingBlock::default();
    svg.apply_override_json(
        &json!({"logoSvg": "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"}),
    );
    assert!(svg.logo_svg.is_some());
    let before = svg.clone();
    svg.apply_override_json(&json!(["not", "an", "object"]));
    assert_eq!(
        svg, before,
        "an override that is not an object changes nothing"
    );
    // unquoted attribute values are read up to the next space
    assert!(dataroom_core::branding::is_safe_logo_svg(
        "<svg width=10 height=10></svg>"
    ));
    assert!(!dataroom_core::branding::is_safe_logo_svg(
        "<svg width=10 onload=x></svg>"
    ));
}

fn png(width: u32, height: u32, ihdr: &[u8; 4]) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\r".to_vec();
    b.extend_from_slice(ihdr);
    b.extend_from_slice(&width.to_be_bytes());
    b.extend_from_slice(&height.to_be_bytes());
    b
}

fn webp_vp8x(width: u32, height: u32, form: &[u8; 4]) -> Vec<u8> {
    let mut b = b"RIFF\x16\0\0\0".to_vec();
    b.extend_from_slice(form);
    b.extend_from_slice(b"VP8X\x0a\0\0\0\0\0\0\0");
    b.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    b.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
    b
}

#[test]
fn raster_logos_are_held_to_their_header_and_the_dimension_cap() {
    // the smallest header that names its size is enough, at the cap and one pixel over it
    assert_eq!(png(4096, 1, b"IHDR").len(), 24);
    assert!(validate_and_sanitise_logo(&png(4096, 4096, b"IHDR"), "image/png").is_ok());
    for (bytes, why) in [
        (png(4097, 10, b"IHDR"), "exceed limit of 4096px"),
        (png(10, 4097, b"IHDR"), "exceed limit of 4096px"),
        (png(0, 10, b"IHDR"), "exceed limit"),
        (png(10, 0, b"IHDR"), "exceed limit"),
        (png(10, 10, b"IDAT"), "invalid PNG header"),
        (png(10, 10, b"IHDR")[..23].to_vec(), "invalid PNG header"),
    ] {
        let err = logo_error(&bytes, "image/png");
        assert!(err.contains(why), "{err}");
    }
    assert_eq!(webp_vp8x(1, 1, b"WEBP").len(), 30);
    assert!(validate_and_sanitise_logo(&webp_vp8x(4096, 4096, b"WEBP"), "image/webp").is_ok());
    for (bytes, why) in [
        (webp_vp8x(4097, 1, b"WEBP"), "exceed limit of 4096px"),
        (webp_vp8x(1, 4097, b"WEBP"), "exceed limit of 4096px"),
        (webp_vp8x(10, 10, b"WAVE"), "invalid WebP header"),
        (
            webp_vp8x(10, 10, b"WEBP")[..29].to_vec(),
            "invalid WebP header",
        ),
    ] {
        let err = logo_error(&bytes, "image/webp");
        assert!(err.contains(why), "{err}");
    }
    let mut not_riff = webp_vp8x(10, 10, b"WEBP");
    not_riff[..4].copy_from_slice(b"RIFX");
    assert!(logo_error(&not_riff, "image/webp").contains("invalid WebP header"));
}

#[test]
fn svg_attributes_named_on_alone_and_animation_targets() {
    let is_safe_logo_svg =
        |svg: &str| validate_and_sanitise_logo(svg.as_bytes(), "image/svg+xml").is_ok();
    assert!(
        is_safe_logo_svg(r#"<svg on="x"></svg>"#),
        "`on` alone is no handler"
    );
    assert!(!is_safe_logo_svg(r#"<svg onx="x"></svg>"#));
    for (target, safe) in [
        ("fill", true),
        ("href", false),
        ("xlink:href", false),
        ("onclick", false),
    ] {
        let svg = format!(r##"<svg><animate attributeName="{target}" to="#a"/></svg>"##);
        assert_eq!(is_safe_logo_svg(&svg), safe, "{target}");
    }
}

#[test]
fn own_dataset_fields_show_only_checked_values_under_their_property() {
    use dataroom_core::branding::BrandingDatasetFields;
    let field = |name: &str, kind: &str| -> jc_core::kinds::DatasetField {
        serde_json::from_value(json!({"name": name, "label": {}, "type": kind})).unwrap()
    };
    let fields = BrandingDatasetFields {
        organization: "city".into(),
        namespace: "https://data.example/ns/dataset-fields/city/".into(),
        fields: vec![
            field("funding", "text"),
            field("budget", "number"),
            field("web", "url"),
        ],
    };
    let extras: BTreeMap<String, String> = [
        ("x_funding", " EU "),
        ("x_budget", "not a number"),
        ("x_web", "  "),
        ("other", "x"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let shown: Vec<_> = fields
        .filled(&extras)
        .map(|(f, v)| (f.name.as_str(), v))
        .collect();
    assert_eq!(shown, [("funding", "EU")]);
    assert_eq!(
        fields.property(&fields.fields[0]).as_deref(),
        Some("https://data.example/ns/dataset-fields/city/funding")
    );
    let bare = BrandingDatasetFields {
        namespace: String::new(),
        ..fields.clone()
    };
    assert_eq!(bare.property(&fields.fields[0]), None);
}
