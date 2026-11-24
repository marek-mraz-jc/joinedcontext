//! Input: a JSON branding block (or any bytes, read as an SVG logo and as colour values).
//! A sanitised block holds only hex colours and a safe logo, whatever came in.
#![no_main]

use kaisatsu_core::branding::{compute_primary_foreground, is_hex_colour, is_safe_svg};
use kaisatsu_core::BrandingBlock;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = is_safe_svg(&text);
    if is_hex_colour(&text) {
        let fg = compute_primary_foreground(&text);
        assert!(is_hex_colour(fg), "foreground {fg} of {text}");
    }
    if let Ok(block) = serde_json::from_slice::<BrandingBlock>(data) {
        let clean = block.sanitised();
        for c in [
            &clean.colours.primary,
            &clean.colours.secondary,
            &clean.colours.accent,
            &clean.colours.background,
        ] {
            assert!(is_hex_colour(c), "sanitised colour {c}");
        }
        let css = clean.css_variables();
        assert!(
            !css.contains("</"),
            "css breaks out of its style element: {css}"
        );
    }
});
