//! The branding of one installation (UI-30, OPS-46, Deployment/12).
//!
//! One image serves every city. What differs is a file: the deployment renders
//! `global.branding` into a ConfigMap, mounts it and names it in `JC_BRANDING_FILE`. Nothing
//! here is a secret, which is why the endpoint that serves it is public and cacheable: the
//! login page needs the instance name and the logo before anyone has signed in.
//!
//! A missing, unreadable or invalid file is not an error. An installation whose ConfigMap has
//! not been rendered yet looks plain rather than failing to load, and the reason is logged.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Proposal validation strictness mode (PF-57).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum Validation {
    #[default]
    Strict,
    Lax,
}

/// Everything the Portal shows that names or themes an installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Branding {
    /// Full name: page titles and the login page.
    pub instance_name: String,
    /// Short name: sidebars, tabs, e-mail subjects. A block that omits it gets the full name,
    /// so the field default is empty rather than the struct's.
    #[serde(default)]
    pub short_name: String,
    /// The city or region this installation serves.
    pub city: String,
    /// The legal publisher, which is also the DCAT-AP `dcterms:publisher`.
    pub organisation: String,
    /// The organization's domain: URN segment, realm display name, `did:web`.
    pub org_domain: String,
    /// The platform host.
    pub domain: String,
    /// The DCAT-AP contact point and the footer's address.
    pub contact_email: String,
    /// The default dataset licence.
    pub license_default: String,
    /// Logo file, served from the platform's own origin.
    pub logo: String,
    /// Favicon file, served from the platform's own origin.
    pub favicon: String,
    /// The colour tokens the UI writes as CSS custom properties.
    pub colours: Colours,
    /// The font stacks the UI writes as CSS custom properties.
    pub fonts: Fonts,
    /// The locales the language switcher offers.
    pub languages: Languages,
    /// Readable text on top of the primary colour. Always computed from that colour, never
    /// taken from the file: the block names a primary colour but no foreground, and white on
    /// a light primary is unreadable (WCAG 1.4.3). A value in the file is overwritten.
    pub primary_foreground: String,
    /// The primary colour as a dark page paints it: the brand lightened towards white, because
    /// a navy button on a navy page is not a button. Computed, never authored.
    pub primary_dark: String,
    /// Readable text on top of `primary_dark` — on the lightened colour, not on the configured
    /// one. The dark theme used to rule that this is always the branded text pushed to black,
    /// which left a `#111827` installation at 2.31:1 and a `#0000bf` one at 3.43:1 (T-2324,
    /// UI-30). Computed here because the choice needs the ratio of the lightened colour.
    pub primary_foreground_dark: String,
    /// Proposal validation strictness (PF-57). In strict mode, proposals require a fresh
    /// green verdict; in lax mode, Green-lane proposals proceed with a warning.
    #[serde(default)]
    pub validation: Validation,
    /// Where this installation serves the User Guide, or empty when it serves none (UI-02,
    /// DP-11). A create form joins it with the page its kind's arrangement names and offers one
    /// link; an installation that leaves it empty shows no link at all, because a dead link is
    /// worse than none. Nothing follows it: it becomes an `href` a person may click and never a
    /// request the Portal makes.
    #[serde(default)]
    pub documentation_base_url: String,
    /// The origin published Apps are served from (`JC_PORTAL_APPS_URL`), absent when they are
    /// served on the Portal's own. Never taken from the branding file: the route overwrites it
    /// from the Portal's configuration on every answer, and the in-Portal page of an App frames
    /// it there (AP-122, T-2840).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps_origin: Option<String>,
    /// The project sections this installation hides (`dashboards` while `JC_PORTAL_DASHBOARDS`
    /// is not `true`, T-2874). Like `apps_origin`, set by the route from the configuration, so
    /// what a branding file says is overwritten; always sent, so the UI never mistakes a shown
    /// section for one it has not heard about.
    pub hidden_sections: Vec<String>,
}

/// The five colours a page is built from. Each is validated as a hex triplet or sextet before
/// it is served, because the UI writes it into a CSS custom property and a value that is not a
/// colour is a way into the page (OPS-46).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Colours {
    /// Primary action colour.
    pub primary: String,
    /// Secondary accent used for links and focus.
    pub secondary: String,
    /// Highlight colour.
    pub accent: String,
    /// Page background.
    pub background: String,
    /// Body text.
    pub text: String,
}

/// Heading and body font stacks. Self-hosted or system families only: nothing on a page
/// fetches from a third-party origin at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Fonts {
    /// Font stack for headings.
    pub heading: String,
    /// Font stack for body text.
    pub body: String,
}

/// The locale the UI starts in and the ones it offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Languages {
    /// The locale a first-time visitor gets.
    pub default: String,
    /// Every locale the switcher lists; always contains the default.
    pub offered: Vec<String>,
}

impl Default for Branding {
    fn default() -> Self {
        Self {
            instance_name: "joinedcontext".into(),
            short_name: "joinedcontext".into(),
            city: String::new(),
            organisation: String::new(),
            org_domain: String::new(),
            domain: String::new(),
            contact_email: String::new(),
            license_default: "CC-BY-4.0".into(),
            logo: String::new(),
            favicon: String::new(),
            colours: Colours::default(),
            fonts: Fonts::default(),
            languages: Languages::default(),
            primary_foreground: "#ffffff".into(),
            primary_dark: "#5985e7".into(),
            primary_foreground_dark: "#0f172a".into(),
            validation: Validation::default(),
            // An installation that says nothing serves no guide, so no form offers a link.
            documentation_base_url: String::new(),
            apps_origin: None,
            hidden_sections: Vec::new(),
        }
    }
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
            offered: vec!["en".into()],
        }
    }
}

impl Branding {
    /// Reads the branding file, falling back to neutral defaults for anything unusable.
    ///
    /// The file is read on every call rather than cached, so applying a new ConfigMap changes
    /// the Portal without a restart (Deployment/12 section 4).
    pub fn load(path: Option<&str>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                tracing::warn!(%path, error = %err, "branding file unreadable, serving defaults");
                return Self::default();
            }
        };
        match serde_norway::from_str::<Self>(&text) {
            Ok(branding) => branding.sanitised(),
            Err(err) => {
                tracing::warn!(%path, error = %err, "branding file is not valid YAML, serving defaults");
                Self::default()
            }
        }
    }

    /// Replaces every value the UI must not be handed with its neutral default.
    pub fn sanitised(mut self) -> Self {
        let fallback = Self::default();
        self.colours = self.colours.sanitised(&fallback.colours);
        self.logo = same_origin(&self.logo, "logo");
        self.favicon = same_origin(&self.favicon, "favicon");
        self.languages = self.languages.sanitised(&fallback.languages);
        self.documentation_base_url = absolute_web_url(&self.documentation_base_url);
        self.primary_foreground = self.primary_foreground().to_owned();
        self.primary_dark = self.primary_dark();
        self.primary_foreground_dark = self.primary_foreground_dark().to_owned();
        if self.instance_name.trim().is_empty() {
            self.instance_name = fallback.instance_name.clone();
        }
        if self.short_name.trim().is_empty() {
            self.short_name = self.instance_name.clone();
        }
        self
    }

    /// The readable text colour on top of the primary colour (WCAG 1.4.3).
    ///
    /// The branding block names a primary colour but no foreground for it, so the readable one
    /// is computed: whichever of the ink and the paper has the **higher contrast ratio** against
    /// the primary. Choosing on luminance alone instead got a band of ordinary brand colours
    /// wrong — the crossover between black and white text sits near a relative luminance of
    /// 0.18, not 0.5, so a mid grey (`#808080`) was given white text at 3.95:1 and `#a0a0a0`
    /// white at 2.61:1, both under the 4.5:1 a page of text needs.
    ///
    /// Some brand colours cannot reach 4.5:1 with either (`#ff0000` reaches 4.46:1 at best).
    /// The better of the two is still served — an unreadable Portal helps nobody — and the
    /// installation is told in the log which colour it was and what it reached.
    /// The primary colour as a dark page paints it: the brand mixed 72 % with white in oklab,
    /// which is what the dark theme's `--portal-brand` used to compute for itself. It is here so
    /// that one place knows both the colour and the text that can be read on it; a brand that is
    /// not a colour at all keeps the neutral default's.
    pub fn primary_dark(&self) -> String {
        mix_with_white(&self.colours.primary, DARK_BRAND_SHARE)
            .unwrap_or_else(|| Self::default().primary_dark)
    }

    /// The readable text colour on top of [`Branding::primary_dark`], by the same rule and the
    /// same two candidates as [`Branding::primary_foreground`].
    pub fn primary_foreground_dark(&self) -> &'static str {
        let dark = self.primary_dark();
        let Some(background) = luminance(&dark) else {
            return FOREGROUND_ON_LIGHT;
        };
        let on_dark = contrast(background, luminance(FOREGROUND_ON_DARK).unwrap_or(1.0));
        let on_light = contrast(background, luminance(FOREGROUND_ON_LIGHT).unwrap_or(0.0));
        if on_dark.max(on_light) < MIN_CONTRAST {
            tracing::warn!(
                primary = %self.colours.primary,
                dark = %dark,
                contrast = on_dark.max(on_light),
                "no text colour reaches {MIN_CONTRAST}:1 on this primary colour in the dark theme; using the better one"
            );
        }
        if on_dark >= on_light {
            FOREGROUND_ON_DARK
        } else {
            FOREGROUND_ON_LIGHT
        }
    }

    pub fn primary_foreground(&self) -> &'static str {
        let Some(primary) = luminance(&self.colours.primary) else {
            return FOREGROUND_ON_DARK;
        };
        let on_dark = contrast(primary, luminance(FOREGROUND_ON_DARK).unwrap_or(1.0));
        let on_light = contrast(primary, luminance(FOREGROUND_ON_LIGHT).unwrap_or(0.0));
        let best = on_dark.max(on_light);
        if best < MIN_CONTRAST {
            tracing::warn!(
                primary = %self.colours.primary,
                contrast = best,
                "no text colour reaches {MIN_CONTRAST}:1 on this primary colour; using the better one"
            );
        }
        if on_dark >= on_light {
            FOREGROUND_ON_DARK
        } else {
            FOREGROUND_ON_LIGHT
        }
    }
}

impl Colours {
    fn sanitised(self, fallback: &Self) -> Self {
        Self {
            primary: hex_or(self.primary, &fallback.primary, "primary"),
            secondary: hex_or(self.secondary, &fallback.secondary, "secondary"),
            accent: hex_or(self.accent, &fallback.accent, "accent"),
            background: hex_or(self.background, &fallback.background, "background"),
            text: hex_or(self.text, &fallback.text, "text"),
        }
    }
}

impl Languages {
    fn sanitised(mut self, fallback: &Self) -> Self {
        self.offered.retain(|locale| is_locale(locale));
        if !is_locale(&self.default) {
            tracing::warn!(locale = %self.default, "branding default locale is not a locale tag");
            self.default = fallback.default.clone();
        }
        // A switcher that cannot reach the locale the page starts in would strand the visitor.
        if !self.offered.iter().any(|locale| locale == &self.default) {
            self.offered.insert(0, self.default.clone());
        }
        self
    }
}

/// A hex triplet or sextet, or the fallback with the reason logged.
fn hex_or(value: String, fallback: &str, field: &'static str) -> String {
    if is_hex_colour(&value) {
        return value;
    }
    tracing::warn!(field, %value, "branding colour is not a hex colour, using the default");
    fallback.to_owned()
}

/// `#rgb` or `#rrggbb`, which is the whole of what may reach a CSS custom property.
pub fn is_hex_colour(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    matches!(digits.len(), 3 | 6) && digits.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A file name served from the platform's own origin: no scheme, no host, no traversal.
fn same_origin(value: &str, field: &'static str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let bad = trimmed.contains("://")
        || trimmed.starts_with("//")
        || trimmed.starts_with('/')
        || trimmed.contains("..")
        || trimmed.contains(char::is_whitespace);
    if bad {
        tracing::warn!(field, value = %trimmed, "branding asset is not a same-origin file name");
        return String::new();
    }
    trimmed.to_owned()
}

/// An absolute `http` or `https` address, or the empty string (UI-02, OPS-46).
///
/// The value reaches the page as an `href`, which is an attribute a browser acts on, so it is
/// checked the way a colour is checked before it becomes a custom property: anything carrying
/// another scheme — `javascript:`, `data:`, `file:` — is dropped and logged rather than served.
/// A trailing slash is removed so the path the arrangement names joins on exactly one.
fn absolute_web_url(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    let scheme = trimmed
        .split_once("://")
        .map(|(scheme, rest)| (scheme.to_ascii_lowercase(), rest));
    match scheme {
        Some((scheme, rest))
            if matches!(scheme.as_str(), "http" | "https")
                && !rest.is_empty()
                && !rest.starts_with('/')
                && !trimmed.contains(char::is_whitespace)
                && !trimmed.contains(['<', '>', '"', '\'']) =>
        {
            trimmed.to_owned()
        }
        _ => {
            tracing::warn!(
                field = "documentationBaseUrl",
                value = %trimmed,
                "branding documentation URL is not an absolute http(s) address, no form will link"
            );
            String::new()
        }
    }
}

/// A BCP 47 tag as far as the switcher needs it: letters, digits and hyphens.
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

/// The text the Portal puts on a dark primary colour, and on a light one.
const FOREGROUND_ON_DARK: &str = "#ffffff";
const FOREGROUND_ON_LIGHT: &str = "#0f172a";

/// What WCAG 1.4.3 asks of text against its background.
const MIN_CONTRAST: f32 = 4.5;

/// How much of the brand is left in the dark theme's version of it. The rest is white, mixed in
/// oklab, which is what `tokens.css` did in CSS until the foreground had to be chosen against
/// the result (T-2324).
const DARK_BRAND_SHARE: f64 = 0.72;

/// `color-mix(in oklab, colour <share>%, white)`, the browser's own arithmetic, so the colour the
/// UI paints and the colour this choice was made against are the same one.
fn mix_with_white(colour: &str, share: f64) -> Option<String> {
    let [r, g, b] = rgb_of(colour)?;
    let mixed = from_oklab(
        to_oklab([r, g, b])
            .iter()
            .zip(to_oklab([255, 255, 255]))
            .map(|(brand, white)| brand * share + white * (1.0 - share))
            .collect::<Vec<_>>()
            .try_into()
            .ok()?,
    );
    Some(format!("#{:02x}{:02x}{:02x}", mixed[0], mixed[1], mixed[2]))
}

/// sRGB to Oklab (Björn Ottosson's matrices), the space `color-mix(in oklab, …)` interpolates in.
///
/// In `f64`, unlike the luminance above: the matrices are published to ten decimals, and a mix
/// that rounds to a byte at the end has no reason to lose them on the way.
pub(crate) fn to_oklab(rgb: [u8; 3]) -> [f64; 3] {
    let linear = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = [linear(rgb[0]), linear(rgb[1]), linear(rgb[2])];
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

/// Oklab back to sRGB, clamped: a mix of two displayable colours can land a hair outside the cube.
pub(crate) fn from_oklab(lab: [f64; 3]) -> [u8; 3] {
    let [big_l, a, b] = lab;
    let l = (big_l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (big_l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (big_l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    let encode = |c: f64| {
        let c = if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (c * 255.0).round().clamp(0.0, 255.0) as u8
    };
    [
        encode(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
        encode(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
        encode(-0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s),
    ]
}

/// The contrast ratio between two relative luminances (WCAG 1.4.3), lighter over darker.
pub(crate) fn contrast(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The three channels of a hex triplet or sextet, or nothing when it is neither.
pub(crate) fn rgb_of(colour: &str) -> Option<[u8; 3]> {
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

/// Relative luminance of a hex colour (WCAG 2.1), for the contrast decision the UI cannot make.
pub(crate) fn luminance(colour: &str) -> Option<f32> {
    let [r, g, b] = rgb_of(colour)?;
    // Each channel is linearised before it is weighted: sRGB is gamma-encoded, and weighting the
    // encoded bytes gives a number that is not a luminance and cannot be compared to a ratio.
    let channel = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = r##"
instanceName: "Banská Bystrica Context"
shortName: "BB Context"
city: "Banská Bystrica"
organisation: "Mesto Banská Bystrica"
orgDomain: "banskabystrica.sk"
domain: "bb.example.com"
contactEmail: "opendata@banskabystrica.sk"
licenseDefault: "CC-BY-4.0"
logo: "logo.svg"
favicon: "favicon.svg"
colours:
  primary: "#0000bf"
  secondary: "#0072c6"
  accent: "#ffe977"
  background: "#ffffff"
  text: "#1a1a1a"
fonts:
  heading: "HelsinkiGrotesk, system-ui, sans-serif"
  body: "system-ui, sans-serif"
languages:
  default: "sk"
  offered: ["sk", "en"]
"##;

    fn written(contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "jc-branding-{}-{}.yaml",
            std::process::id(),
            contents.len()
        ));
        std::fs::write(&path, contents).expect("write the branding file");
        path
    }

    #[test]
    fn no_branding_file_is_neutral_defaults() {
        let branding = Branding::load(None);
        assert_eq!(branding.instance_name, "joinedcontext");
        assert_eq!(branding.colours.primary, "#1d4ed8");
        assert_eq!(branding.languages.offered, vec!["en".to_string()]);
    }

    #[test]
    fn an_unreadable_file_is_neutral_defaults() {
        assert_eq!(
            Branding::load(Some("/nonexistent/jc/branding.yaml")),
            Branding::default()
        );
    }

    #[test]
    fn an_invalid_file_is_neutral_defaults() {
        let path = written("instanceName: [unclosed\n");
        let branding = Branding::load(Some(&path.display().to_string()));
        let _ = std::fs::remove_file(&path);
        assert_eq!(branding, Branding::default());
    }

    #[test]
    fn the_documented_block_is_served_as_written() {
        let path = written(BLOCK);
        let branding = Branding::load(Some(&path.display().to_string()));
        let _ = std::fs::remove_file(&path);

        assert_eq!(branding.instance_name, "Banská Bystrica Context");
        assert_eq!(branding.organisation, "Mesto Banská Bystrica");
        assert_eq!(branding.colours.primary, "#0000bf");
        assert_eq!(
            branding.fonts.heading,
            "HelsinkiGrotesk, system-ui, sans-serif"
        );
        assert_eq!(branding.languages.default, "sk");
        assert_eq!(
            branding.languages.offered,
            vec!["sk".to_string(), "en".into()]
        );
        assert_eq!(branding.logo, "logo.svg");
    }

    #[test]
    fn a_partial_block_keeps_the_defaults_for_what_it_omits() {
        let path = written("instanceName: \"Helsinki Region Context\"\n");
        let branding = Branding::load(Some(&path.display().to_string()));
        let _ = std::fs::remove_file(&path);

        assert_eq!(branding.instance_name, "Helsinki Region Context");
        // A block that names no short name gets the full one rather than "joinedcontext".
        assert_eq!(branding.short_name, "Helsinki Region Context");
        assert_eq!(branding.colours, Colours::default());
    }

    /// OPS-46: a custom property is a value the browser evaluates.
    #[test]
    fn a_colour_that_is_not_a_colour_is_replaced_by_the_default() {
        for attempt in [
            "red; background: url(https://evil.example/x)",
            "var(--anything)",
            "#12",
            "#1234567",
            "#gggggg",
            "",
        ] {
            let branding = Branding {
                colours: Colours {
                    primary: attempt.to_owned(),
                    ..Colours::default()
                },
                ..Branding::default()
            }
            .sanitised();
            assert_eq!(
                branding.colours.primary,
                Colours::default().primary,
                "`{attempt}` must not reach a style"
            );
        }
        assert!(is_hex_colour("#fff") && is_hex_colour("#0000BF"));
    }

    #[test]
    fn an_asset_from_another_origin_is_dropped() {
        for attempt in [
            "https://evil.example/logo.svg",
            "//evil.example/logo.svg",
            "../../etc/passwd",
            "/etc/passwd",
        ] {
            let branding = Branding {
                logo: attempt.to_owned(),
                ..Branding::default()
            }
            .sanitised();
            assert!(branding.logo.is_empty(), "`{attempt}` must not be served");
        }
    }

    #[test]
    fn the_switcher_always_reaches_the_starting_locale() {
        let branding = Branding {
            languages: Languages {
                default: "fi".into(),
                offered: vec!["sv".into(), "en".into()],
            },
            ..Branding::default()
        }
        .sanitised();

        assert_eq!(branding.languages.offered, vec!["fi", "sv", "en"]);
    }

    #[test]
    fn a_light_primary_gets_dark_text_on_it() {
        let light = Branding {
            colours: Colours {
                primary: "#ffe977".into(),
                ..Colours::default()
            },
            ..Branding::default()
        };
        assert_eq!(light.primary_foreground(), "#0f172a");
        assert_eq!(Branding::default().primary_foreground(), "#ffffff");
    }

    fn with_primary(primary: &str) -> Branding {
        Branding {
            colours: Colours {
                primary: primary.into(),
                ..Colours::default()
            },
            ..Branding::default()
        }
    }

    /// The text on the primary colour is whichever reaches further, measured, not guessed.
    #[test]
    fn the_text_on_a_primary_colour_is_the_one_that_reads_on_it() {
        // Every one of these was given white text by the old luminance threshold, and every one
        // of them was unreadable with it: #808080 reached 3.95:1 and #a0a0a0 only 2.61:1.
        for primary in [
            "#808080", "#a0a0a0", "#c0c0c0", "#ff0000", "#00ff00", "#ffff00",
        ] {
            let branding = with_primary(primary);
            let chosen = branding.primary_foreground();
            let background = luminance(primary).expect("a hex colour");
            let got = contrast(background, luminance(chosen).expect("a hex colour"));
            let other = contrast(
                background,
                luminance(if chosen == FOREGROUND_ON_DARK {
                    FOREGROUND_ON_LIGHT
                } else {
                    FOREGROUND_ON_DARK
                })
                .expect("a hex colour"),
            );
            assert!(
                got >= other,
                "{primary}: chose {chosen} at {got:.2}:1 over {other:.2}:1"
            );
        }

        // A dark brand still gets white, a light one still gets ink: the ordinary cases do not
        // move because the measure changed.
        assert_eq!(with_primary("#1d4ed8").primary_foreground(), "#ffffff");
        assert_eq!(with_primary("#0f172a").primary_foreground(), "#ffffff");
        assert_eq!(with_primary("#ffffff").primary_foreground(), "#0f172a");
        assert_eq!(with_primary("#ffe977").primary_foreground(), "#0f172a");
    }

    /// A brand that no text reads well on is served its best, never a worse one (WCAG 1.4.3).
    #[test]
    fn a_primary_colour_no_text_reads_on_still_gets_the_better_of_the_two() {
        let branding = with_primary("#ff0000");
        let chosen = branding.primary_foreground();
        let background = luminance("#ff0000").expect("a hex colour");
        // Neither reaches 4.5:1 on pure red; the darker one reaches further.
        assert_eq!(chosen, FOREGROUND_ON_LIGHT);
        assert!(contrast(background, luminance(chosen).unwrap()) > 4.4);
        assert!(contrast(background, luminance(FOREGROUND_ON_DARK).unwrap()) < 4.1);
    }

    /// The dark theme's pair is the same choice, made against the colour the dark theme paints.
    #[test]
    fn the_dark_themes_text_reads_on_the_dark_themes_button() {
        // Every brand an installation is likely to set, including the two that were unreadable
        // when the dark theme ruled that the label is always the branded text pushed to black:
        // `#111827` at 2.31:1 and Banská Bystrica's `#0000bf` at 3.43:1 (T-2324).
        for primary in [
            "#1d4ed8", "#0000bf", "#111827", "#7dd3fc", "#dc2626", "#ffe977", "#000000", "#ffffff",
        ] {
            let branding = with_primary(primary);
            let dark = branding.primary_dark();
            let chosen = branding.primary_foreground_dark();
            let background = luminance(&dark).expect("a mix of two hex colours is one");
            let got = contrast(background, luminance(chosen).expect("a hex colour"));
            assert!(
                got >= MIN_CONTRAST,
                "{primary}: {chosen} on {dark} is only {got:.2}:1"
            );
        }
    }

    /// The mix is the browser's, or the colour chosen against is not the colour painted.
    #[test]
    fn the_dark_primary_is_the_oklab_mix_the_stylesheet_used_to_compute() {
        // Measured with `ui/tests/tokenContrast.ts`, which resolves `color-mix(in oklab, …)` the
        // way a browser does: these are the colours the dark theme already paints today, so
        // nothing an installation looks at moves except the label on a dark brand.
        assert_eq!(with_primary("#1d4ed8").primary_dark(), "#5985e7");
        assert_eq!(with_primary("#7dd3fc").primary_dark(), "#a5e0fd");
        assert_eq!(with_primary("#111827").primary_dark(), "#4a505d");
        assert_eq!(with_primary("#dc2626").primary_dark(), "#ee7266");
        assert_eq!(with_primary("#0000bf").primary_dark(), "#3666d7");
        // White stays white, and the short form is the long one.
        assert_eq!(with_primary("#ffffff").primary_dark(), "#ffffff");
        assert_eq!(with_primary("#fff").primary_dark(), "#ffffff");
        // A brand that is not a colour never reaches the mix, but if one did the page still gets
        // a colour rather than a broken custom property.
        assert_eq!(
            with_primary("url(evil)").primary_dark(),
            Branding::default().primary_dark
        );
    }

    /// Both are computed, so a block that names them cannot put unreadable text on a button.
    #[test]
    fn the_dark_pair_in_the_file_is_overwritten() {
        let branding = Branding {
            primary_dark: "#000000".into(),
            primary_foreground_dark: "#010101".into(),
            ..with_primary("#111827")
        }
        .sanitised();

        assert_eq!(branding.primary_dark, "#4a505d");
        assert_eq!(branding.primary_foreground_dark, "#ffffff");
        // And the light pair is still chosen for the configured colour, not the lightened one.
        assert_eq!(branding.primary_foreground, "#ffffff");
    }

    /// The luminance is WCAG's, so it can be compared against a ratio at all.
    #[test]
    fn luminance_is_linearised_before_it_is_weighted() {
        // Byte-weighted "luminance" would put mid grey at 0.5; linearised it is near 0.216.
        let grey = luminance("#808080").expect("a hex colour");
        assert!((0.21..0.22).contains(&grey), "{grey}");
        assert_eq!(luminance("#ffffff"), Some(1.0));
        assert_eq!(luminance("#000000"), Some(0.0));
        // The short form is the long one.
        assert_eq!(luminance("#fff"), luminance("#ffffff"));
        assert_eq!(luminance("not a colour"), None);
        // White on black is the widest a screen goes.
        assert!((contrast(1.0, 0.0) - 21.0).abs() < 0.001);
    }

    /// UI-02, OPS-46: the documentation URL becomes an `href`, so only an absolute `http` or
    /// `https` address survives; anything else is dropped and the forms show no link.
    #[test]
    fn a_documentation_url_that_is_not_an_absolute_web_address_is_dropped() {
        // An installation that says nothing serves no guide.
        assert_eq!(Branding::default().documentation_base_url, "");
        let quiet: Branding = serde_norway::from_str("instanceName: \"Test\"\n").unwrap();
        assert_eq!(quiet.sanitised().documentation_base_url, "");

        for value in [
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            "vbscript:msgbox(1)",
            "docs.example.com",
            "/docs",
            "//docs.example.com",
            "https://",
            "https:///docs",
            "https://docs.example.com/a b",
            "https://docs.example.com/\"onmouseover=\"alert(1)",
            "  ",
        ] {
            let branding = Branding {
                documentation_base_url: value.to_owned(),
                ..Branding::default()
            }
            .sanitised();
            assert_eq!(
                branding.documentation_base_url, "",
                "{value:?} reached the page"
            );
        }

        for (value, served) in [
            ("https://docs.example.com", "https://docs.example.com"),
            ("https://docs.example.com/", "https://docs.example.com"),
            // The trailing slash goes, so the arrangement's path joins on exactly one.
            (
                "https://docs.example.com/guide///",
                "https://docs.example.com/guide",
            ),
            (
                "  https://docs.example.com/guide  ",
                "https://docs.example.com/guide",
            ),
            ("HTTPS://docs.example.com", "HTTPS://docs.example.com"),
            ("http://localhost:3000", "http://localhost:3000"),
        ] {
            let branding = Branding {
                documentation_base_url: value.to_owned(),
                ..Branding::default()
            }
            .sanitised();
            assert_eq!(branding.documentation_base_url, served, "{value:?}");
        }
    }

    #[test]
    fn validation_mode_defaults_to_strict_and_parses_lax() {
        assert_eq!(Branding::default().validation, Validation::Strict);

        let yaml = "instanceName: \"Test\"\nvalidation: lax\n";
        let branding: Branding = serde_norway::from_str(yaml).unwrap();
        assert_eq!(branding.validation, Validation::Lax);

        let yaml_strict = "instanceName: \"Test\"\nvalidation: strict\n";
        let branding_strict: Branding = serde_norway::from_str(yaml_strict).unwrap();
        assert_eq!(branding_strict.validation, Validation::Strict);
    }
}
