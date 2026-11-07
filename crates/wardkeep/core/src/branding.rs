#![forbid(unsafe_code)]
//! Per-realm and per-organization branding block (W7), matching the deployment's `global.branding`
//! and the Portal's `src/branding.rs` with WCAG 1.4.3 contrast computation and SVG sanitisation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct BrandingBlock {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub instance_name: String,
    pub short_name: String,
    pub city: String,
    pub organisation: String,
    pub org_domain: String,
    pub domain: String,
    pub contact_email: String,
    pub logo: String,
    pub logo_svg: String,
    pub favicon: String,
    pub colours: Colours,
    pub fonts: Fonts,
    pub languages: Languages,
    pub tagline: String,
    pub footer_lines: Vec<String>,
    pub ribbon: String,
    pub primary_foreground: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Colours {
    pub primary: String,
    pub secondary: String,
    pub accent: String,
    pub background: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Fonts {
    pub heading: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Languages {
    pub default: String,
    pub offered: Vec<String>,
}

impl Default for Colours {
    fn default() -> Self {
        Self {
            primary: "#1d4ed8".into(),
            secondary: "#0f766e".into(),
            accent: "#f59e0b".into(),
            background: "#ffffff".into(),
            text: "#0f172a".into(),
        }
    }
}

impl Default for Fonts {
    fn default() -> Self {
        Self {
            heading: "system-ui, sans-serif".into(),
            body: "system-ui, sans-serif".into(),
        }
    }
}

impl Default for Languages {
    fn default() -> Self {
        Self {
            default: "en".into(),
            offered: vec![
                "en".into(),
                "sk".into(),
                "cs".into(),
                "de".into(),
                "hu".into(),
            ],
        }
    }
}

impl Default for BrandingBlock {
    fn default() -> Self {
        Self {
            id: "default".into(),
            realm_id: "master".into(),
            org_id: None,
            instance_name: "joinedcontext".into(),
            short_name: "joinedcontext".into(),
            city: String::new(),
            organisation: "joinedcontext".into(),
            org_domain: String::new(),
            domain: String::new(),
            contact_email: String::new(),
            logo: String::new(),
            logo_svg: String::new(),
            favicon: String::new(),
            colours: Colours::default(),
            fonts: Fonts::default(),
            languages: Languages::default(),
            tagline: String::new(),
            footer_lines: Vec::new(),
            ribbon: String::new(),
            primary_foreground: "#ffffff".into(),
        }
    }
}

impl BrandingBlock {
    /// Sanitises every field, falling back to neutral defaults instead of failing (W7).
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        let fb = Self::default();
        self.colours = Colours {
            primary: hex_or(&self.colours.primary, &fb.colours.primary),
            secondary: hex_or(&self.colours.secondary, &fb.colours.secondary),
            accent: hex_or(&self.colours.accent, &fb.colours.accent),
            background: hex_or(&self.colours.background, &fb.colours.background),
            text: hex_or(&self.colours.text, &fb.colours.text),
        };
        self.fonts = Fonts {
            heading: font_or(&self.fonts.heading, &fb.fonts.heading),
            body: font_or(&self.fonts.body, &fb.fonts.body),
        };
        self.logo = asset_name(&self.logo);
        self.favicon = asset_name(&self.favicon);
        if !is_safe_svg(&self.logo_svg) {
            self.logo_svg.clear();
        }
        self.languages.offered.retain(|l| is_locale(l));
        if !is_locale(&self.languages.default) {
            self.languages.default = fb.languages.default;
        }
        if !self.languages.offered.contains(&self.languages.default) {
            self.languages
                .offered
                .insert(0, self.languages.default.clone());
        }
        if self.instance_name.trim().is_empty() {
            self.instance_name = fb.instance_name;
        }
        if self.short_name.trim().is_empty() {
            self.short_name = self.instance_name.clone();
        }
        if self.organisation.trim().is_empty() {
            self.organisation = self.instance_name.clone();
        }
        self.primary_foreground = compute_primary_foreground(&self.colours.primary).to_owned();
        self
    }

    pub fn css_variables(&self) -> String {
        let s = self.clone().sanitised();
        format!(
            ":root {{\n  --jc-primary: {};\n  --jc-primary-fg: {};\n  --jc-secondary: {};\n  --jc-accent: {};\n  --jc-background: {};\n  --jc-text: {};\n  --jc-font-heading: {};\n  --jc-font-body: {};\n}}",
            s.colours.primary,
            s.primary_foreground,
            s.colours.secondary,
            s.colours.accent,
            s.colours.background,
            s.colours.text,
            s.fonts.heading,
            s.fonts.body,
        )
    }
}

pub fn is_hex_colour(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    matches!(digits.len(), 3 | 6) && digits.bytes().all(|b| b.is_ascii_hexdigit())
}

fn hex_or(value: &str, fallback: &str) -> String {
    if is_hex_colour(value) {
        value.to_owned()
    } else {
        fallback.to_owned()
    }
}

fn font_or(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if !trimmed.is_empty()
        && trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b',' | b'\'' | b'"' | b'-'))
    {
        trimmed.to_owned()
    } else {
        fallback.to_owned()
    }
}

fn asset_name(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.contains("://")
        || trimmed.starts_with('/')
        || trimmed.contains("..")
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        String::new()
    } else {
        trimmed.to_owned()
    }
}

fn is_locale(value: &str) -> bool {
    (2..=12).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic())
}

/// Checks that an SVG contains no script, foreignObject, event handler, or external href (W7, W9).
pub fn is_safe_svg(svg: &str) -> bool {
    if svg.is_empty() {
        return true;
    }
    let lower = svg.to_ascii_lowercase();
    for forbidden in [
        "<script",
        "<foreignobject",
        "<iframe",
        "<embed",
        "<object",
        "<handler",
        "<!",
        "javascript:",
        "&#",
        "onload",
        "onerror",
        "onclick",
        "onmouseover",
    ] {
        if lower.contains(forbidden) {
            return false;
        }
    }
    true
}

fn rgb_of(colour: &str) -> Option<[u8; 3]> {
    let digits = colour.strip_prefix('#')?;
    let expand = |c: u8| u8::from_str_radix(&format!("{}{}", c as char, c as char), 16).ok();
    match digits.len() {
        3 => {
            let d = digits.as_bytes();
            Some([expand(d[0])?, expand(d[1])?, expand(d[2])?])
        }
        6 => Some([
            u8::from_str_radix(&digits[0..2], 16).ok()?,
            u8::from_str_radix(&digits[2..4], 16).ok()?,
            u8::from_str_radix(&digits[4..6], 16).ok()?,
        ]),
        _ => None,
    }
}

fn luminance(colour: &str) -> Option<f32> {
    let [r, g, b] = rgb_of(colour)?;
    let ch = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b))
}

fn contrast(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub fn compute_primary_foreground(primary: &str) -> &'static str {
    let Some(lum) = luminance(primary) else {
        return "#ffffff";
    };
    let on_white = contrast(lum, 1.0);
    let on_dark = contrast(lum, luminance("#0f172a").unwrap_or(0.0));
    if on_white >= on_dark {
        "#ffffff"
    } else {
        "#0f172a"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_branding_values_fall_back_to_neutral_defaults() {
        let dirty = BrandingBlock {
            colours: Colours {
                primary: "javascript:alert(1)".into(),
                secondary: "#0f766e".into(),
                accent: "not-a-color".into(),
                background: "#fff".into(),
                text: "#000000".into(),
            },
            fonts: Fonts {
                heading: "Inter; background: url(evil)".into(),
                body: "system-ui, sans-serif".into(),
            },
            logo: "https://evil.example/logo.svg".into(),
            logo_svg: "<svg onload=alert(1)></svg>".into(),
            ..BrandingBlock::default()
        };
        let clean = dirty.sanitised();
        assert_eq!(clean.colours.primary, "#1d4ed8");
        assert_eq!(clean.colours.accent, "#f59e0b");
        assert_eq!(clean.colours.background, "#fff");
        assert_eq!(clean.fonts.heading, "system-ui, sans-serif");
        assert!(clean.logo.is_empty());
        assert!(clean.logo_svg.is_empty());
        assert_eq!(clean.primary_foreground, "#ffffff");
    }
}
