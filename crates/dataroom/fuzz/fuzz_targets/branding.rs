//! Input: one byte that picks a reader, then the text an admin or a portal file hands it: a
//! branding override (JSON or YAML), a font stack, a colour, a base URL, a host name, an MCP
//! server id or a logo file name. Each must answer a value or a refusal; what it accepts must hold
//! the property its checks promise.
#![no_main]

use dataroom_core::branding::{
    is_base_url, is_host_name, is_mcp_server, is_safe_logo_filename, is_valid_hex_colour,
    origin_of, parse_rgb, sanitize_font_stack, BrandingBlock,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((pick, rest)) = data.split_first() else {
        return;
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    match pick % 6 {
        0 | 1 => {
            let b = if pick % 6 == 0 {
                BrandingBlock::from_str_lossy(text)
            } else {
                BrandingBlock::from_yaml_lossy(text)
            };
            // every colour the block keeps is one a stylesheet can carry as is
            for c in [&b.colours.primary, &b.colours.secondary, &b.colours.accent] {
                assert!(c.is_empty() || is_valid_hex_colour(c), "colour kept: {c:?}");
            }
            let css = b.layout.tokens_css();
            assert!(!css.contains(['<', '{', '}', '"']), "css: {css:?}");
        }
        2 => {
            if let Some(stack) = sanitize_font_stack(text) {
                assert!(
                    !stack.contains([';', '{', '}', '<', '>']),
                    "font: {stack:?}"
                );
                assert!(
                    !stack.to_ascii_lowercase().contains("url("),
                    "font: {stack:?}"
                );
            }
        }
        3 => {
            if parse_rgb(text).is_some() {
                assert!(is_valid_hex_colour(text));
            }
        }
        4 => {
            if is_base_url(text) {
                let origin = origin_of(text);
                assert!(text.starts_with(origin), "origin {origin:?} of {text:?}");
            }
            let _ = is_host_name(text);
            let _ = is_mcp_server(text);
        }
        _ => {
            if is_safe_logo_filename(text) {
                assert!(!text.contains(['/', '\\']), "{text:?}");
            }
        }
    }
});
