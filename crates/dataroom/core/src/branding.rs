#![forbid(unsafe_code)]
//! Branding block (`global.branding` shape from `ours/ckan-theme/plugin.py`, B1-B4, C18)
//! with three-level inheritance (`installation` -> `catalogue` -> `publisher`), WCAG AA
//! contrast checking, and SVG/PNG/WebP logo upload sanitization + content-hash addressing.

use crate::error::{DataroomError, Result};
use crate::model::{text_in_lang, Texts};
use base64::Engine as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Neutral defaults matching `NEUTRAL` in `ours/ckan-theme/plugin.py` lines 24-48.
pub const DEFAULT_PRIMARY: &str = "#1d4ed8";
pub const DEFAULT_SECONDARY: &str = "#0f766e";
pub const DEFAULT_ACCENT: &str = "#f59e0b";
pub const DEFAULT_BACKGROUND: &str = "#ffffff";
pub const DEFAULT_TEXT: &str = "#0f172a";
pub const DEFAULT_FONT_STACK: &str = "system-ui, sans-serif";

/// Colour tokens of a branding block (`colours` in `branding.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrandingColours {
    pub primary: String,
    pub secondary: String,
    pub accent: String,
    pub background: String,
    pub text: String,
}

impl Default for BrandingColours {
    fn default() -> Self {
        Self {
            primary: DEFAULT_PRIMARY.into(),
            secondary: DEFAULT_SECONDARY.into(),
            accent: DEFAULT_ACCENT.into(),
            background: DEFAULT_BACKGROUND.into(),
            text: DEFAULT_TEXT.into(),
        }
    }
}

/// Font stacks of a branding block (`fonts` in `branding.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrandingFonts {
    pub heading: String,
    pub body: String,
}

impl Default for BrandingFonts {
    fn default() -> Self {
        Self {
            heading: DEFAULT_FONT_STACK.into(),
            body: DEFAULT_FONT_STACK.into(),
        }
    }
}

/// Language settings of a branding block (`languages` in `branding.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrandingLanguages {
    pub default: String,
    pub offered: Vec<String>,
}

impl Default for BrandingLanguages {
    fn default() -> Self {
        Self {
            default: "en".into(),
            offered: vec!["en".into()],
        }
    }
}

/// Complete validated branding block matching `ours/ckan-theme/plugin.py` `BRANDING` and
/// `ours/environments/{dev,stage,smartcity}-branding.yaml` (B1, G4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrandingBlock {
    pub instance_name: String,
    pub short_name: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub organisation: String,
    #[serde(default)]
    pub org_domain: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub contact_email: String,
    #[serde(default)]
    pub documentation_base_url: String,
    #[serde(default)]
    pub license_default: String,
    #[serde(default)]
    pub logo: String,
    #[serde(default)]
    pub favicon: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_svg: Option<String>,
    pub colours: BrandingColours,
    pub fonts: BrandingFonts,
    pub languages: BrandingLanguages,
    pub primary_foreground: String,
    pub primary_rgb: String,
    #[serde(default)]
    pub tagline: String,
    #[serde(default)]
    pub footer_lines: Vec<String>,
    #[serde(default)]
    pub ribbon: String,
    #[serde(default)]
    pub noindex: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub site_title: Texts,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub site_tagline: Texts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_logo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_issued: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_server: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub samples: BTreeMap<String, Vec<String>>,
    /// The assistant widget a host installation frames (`jc`, G2): `Some(url)` docks that page,
    /// `Some("")` shows no launcher, `None` keeps `https://assistant.{domain}/d/data/widget`. Only
    /// the host sets it: serde skips it, so no catalogue, override or API reads or writes it.
    #[serde(skip)]
    pub assistant_widget: Option<String>,
    /// The live parts' bases one by one; each left empty falls back to `{label}.{domain}`.
    #[serde(default, skip_serializing_if = "BrandingAddresses::is_empty")]
    pub addresses: BrandingAddresses,
}

/// Where the live parts of a catalogue answer (G1), each a base URL without a trailing slash
/// ([`is_base_url`]) or empty: the chart and explorer host, the gateway (`/api/endpoint/...`,
/// `/api/mcp`), the basemap tiles, the assistant (`/d/data/widget`) and the Portal. An
/// installation without a `domain` (jc) names each one it serves; one it does not serve stays
/// empty, and the page shows nothing that would point at it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct BrandingAddresses {
    pub viz: String,
    pub api: String,
    pub tiles: String,
    pub assistant: String,
    pub portal: String,
}

impl BrandingAddresses {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// One live part of [`BrandingAddresses`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Address {
    Viz,
    Api,
    Tiles,
    Assistant,
    Portal,
}

impl Address {
    pub const ALL: [Address; 5] = [
        Address::Viz,
        Address::Api,
        Address::Tiles,
        Address::Assistant,
        Address::Portal,
    ];

    /// The key in `addresses` and the host label under `domain`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Address::Viz => "viz",
            Address::Api => "api",
            Address::Tiles => "tiles",
            Address::Assistant => "assistant",
            Address::Portal => "portal",
        }
    }

    fn of(self, a: &BrandingAddresses) -> &str {
        match self {
            Address::Viz => &a.viz,
            Address::Api => &a.api,
            Address::Tiles => &a.tiles,
            Address::Assistant => &a.assistant,
            Address::Portal => &a.portal,
        }
    }

    fn of_mut(self, a: &mut BrandingAddresses) -> &mut String {
        match self {
            Address::Viz => &mut a.viz,
            Address::Api => &mut a.api,
            Address::Tiles => &mut a.tiles,
            Address::Assistant => &mut a.assistant,
            Address::Portal => &mut a.portal,
        }
    }
}

/// A base URL a page may build links, frames and its CSP from: `http(s)://host[:port][/path]`,
/// a lowercase host name, no user, query, fragment, quote or trailing slash.
#[must_use]
pub fn is_base_url(v: &str) -> bool {
    let Some(rest) = v
        .strip_prefix("https://")
        .or_else(|| v.strip_prefix("http://"))
    else {
        return false;
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    // One label or more (`localhost`, a container name, `api.city.example`), as jc's own check.
    let host_ok = host.split('.').all(|l| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    });
    let port_ok =
        port.is_none_or(|p| (1..=5).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()));
    let path_ok = !path.ends_with('/')
        && path.bytes().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~')
        })
        && !path.contains("//")
        && !path.split('/').any(|seg| seg == "." || seg == "..");
    host_ok && port_ok && path_ok
}

/// The origin (`scheme://host[:port]`) of a [`is_base_url`] value.
#[must_use]
pub fn origin_of(base: &str) -> &str {
    let after = base.find("://").map_or(0, |i| i + 3);
    base[after..]
        .find('/')
        .map_or(base, |end| &base[..after + end])
}

impl Default for BrandingBlock {
    fn default() -> Self {
        let colours = BrandingColours::default();
        let (r, g, b) = parse_rgb(&colours.primary).unwrap_or((29, 78, 216));
        Self {
            instance_name: "joinedcontext".into(),
            short_name: "joinedcontext".into(),
            city: String::new(),
            organisation: String::new(),
            org_domain: String::new(),
            domain: String::new(),
            contact_email: String::new(),
            documentation_base_url: String::new(),
            license_default: String::new(),
            logo: String::new(),
            favicon: String::new(),
            logo_svg: None,
            colours,
            fonts: BrandingFonts::default(),
            languages: BrandingLanguages::default(),
            primary_foreground: yiq_foreground(r, g, b).into(),
            primary_rgb: format!("{r}, {g}, {b}"),
            tagline: String::new(),
            footer_lines: Vec::new(),
            ribbon: String::new(),
            noindex: false,
            site_title: BTreeMap::new(),
            site_tagline: BTreeMap::new(),
            site_logo: None,
            site_issued: None,
            mcp_server: None,
            samples: BTreeMap::new(),
            assistant_widget: None,
            addresses: BrandingAddresses::default(),
        }
    }
}

/// Checks whether `value` is `#rgb` or `#rrggbb` of hex digits (`_is_colour` in `plugin.py`).
pub fn is_valid_hex_colour(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes.len(), 4 | 7) && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

/// Parses a `#rgb` or `#rrggbb` colour into `(r, g, b)` (`_rgb` in `plugin.py`).
pub fn parse_rgb(colour: &str) -> Option<(u8, u8, u8)> {
    if !is_valid_hex_colour(colour) {
        return None;
    }
    let hex = &colour[1..];
    if hex.len() == 3 {
        let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
        let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
        let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
        Some((r, g, b))
    } else {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some((r, g, b))
    }
}

/// Computes `#000000` or `#ffffff` foreground colour on `(r, g, b)` using the YIQ formula from
/// `plugin.py` line 90: `(r * 299 + g * 587 + b * 114) // 1000 >= 128`.
pub const fn yiq_foreground(r: u8, g: u8, b: u8) -> &'static str {
    let yiq = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
    if yiq >= 128 {
        "#000000"
    } else {
        "#ffffff"
    }
}

/// Sanitizes a font stack so no `url(...)`, `@import`, `;`, `{`, `}` or third-party origin can
/// reach a CSS custom property (B2).
pub fn sanitize_font_stack(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("url(")
        || lower.contains("http:")
        || lower.contains("https:")
        || lower.contains("@import")
        || trimmed.contains([';', '{', '}', '<', '>'])
    {
        return None;
    }
    Some(trimmed.to_owned())
}

impl BrandingBlock {
    /// Loads and validates a branding block from an arbitrary JSON `Value` using the exact
    /// fallback rules of `plugin.py` `_load`: a non-object or broken value falls back to `NEUTRAL`
    /// without erroring (B1).
    pub fn from_json_lossy(raw: &Value) -> Self {
        let mut b = Self::default();
        b.apply_override_json(raw);
        b
    }

    /// Parses a JSON string or YAML-like `branding:` block lossily; never errors (B1).
    pub fn from_str_lossy(raw: &str) -> Self {
        if let Ok(val) = serde_json::from_str::<Value>(raw) {
            return Self::from_json_lossy(&val);
        }
        Self::from_yaml_lossy(raw)
    }

    /// Parses the `ours/environments/{dev,stage,smartcity}-branding.yaml` blocks lossily (B1).
    pub fn from_yaml_lossy(yaml: &str) -> Self {
        let mut obj = serde_json::Map::new();
        let mut colours = serde_json::Map::new();
        let mut fonts = serde_json::Map::new();
        let mut offered = Vec::new();
        let mut default_lang = None;
        let mut section = "";

        for raw_line in yaml.lines() {
            let trimmed = raw_line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("{{") {
                continue;
            }
            let indent = raw_line.len() - raw_line.trim_start().len();
            if trimmed == "branding:" {
                section = "root";
                continue;
            }
            if indent <= 4 && trimmed.ends_with(':') {
                section = match trimmed.trim_end_matches(':') {
                    "colours" => "colours",
                    "fonts" => "fonts",
                    "languages" => "languages",
                    _ => "other",
                };
                continue;
            }
            if section == "languages" && trimmed.starts_with("- ") {
                let code = trimmed
                    .trim_start_matches("- ")
                    .trim()
                    .trim_matches(['\'', '"']);
                if !code.is_empty() {
                    offered.push(Value::String(code.to_owned()));
                }
                continue;
            }
            let Some((k, v_raw)) = trimmed.split_once(':') else {
                continue;
            };
            let key = k.trim();
            let mut val = v_raw.trim().trim_matches(['\'', '"']).to_owned();
            // Resolve Helm `{{ env "..." | default "..." }}` expressions in dev-branding.yaml
            if let Some(def_idx) = val.find("default \"") {
                let after = &val[def_idx + "default \"".len()..];
                if let Some(end_q) = after.find('"') {
                    let def_val = &after[..end_q];
                    if val.starts_with("https://{{") {
                        if let Some(close_br) = val.find("}}") {
                            val = format!("https://{def_val}{}", &val[close_br + 2..]);
                        }
                    } else {
                        val = def_val.to_owned();
                    }
                }
            }
            if val.is_empty() || val == "|" {
                continue;
            }
            match section {
                "colours" => {
                    colours.insert(key.to_owned(), Value::String(val));
                }
                "fonts" => {
                    fonts.insert(key.to_owned(), Value::String(val));
                }
                "languages" => {
                    if key == "default" {
                        default_lang = Some(val);
                    }
                }
                _ => {
                    if key == "noindex" {
                        obj.insert(key.to_owned(), Value::Bool(val == "true"));
                    } else {
                        obj.insert(key.to_owned(), Value::String(val));
                    }
                }
            }
        }
        if !colours.is_empty() {
            obj.insert("colours".into(), Value::Object(colours));
        }
        if !fonts.is_empty() {
            obj.insert("fonts".into(), Value::Object(fonts));
        }
        if default_lang.is_some() || !offered.is_empty() {
            let mut l = serde_json::Map::new();
            if let Some(dl) = default_lang {
                l.insert("default".into(), Value::String(dl));
            }
            if !offered.is_empty() {
                l.insert("offered".into(), Value::Array(offered));
            }
            obj.insert("languages".into(), Value::Object(l));
        }
        Self::from_json_lossy(&Value::Object(obj))
    }

    /// Layers another branding JSON object on top of `self` (`installation -> catalogue` override, B1).
    /// Invalid colour strings or malformed fields are ignored and keep the previous level's value.
    pub fn apply_override_json(&mut self, raw: &Value) {
        let Some(obj) = raw.as_object() else {
            return;
        };
        let str_field = |key: &str| -> Option<String> {
            obj.get(key)
                .and_then(Value::as_str)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        };

        if let Some(v) = str_field("instanceName").or_else(|| str_field("instance_name")) {
            self.instance_name = v;
        }
        if let Some(v) = str_field("shortName").or_else(|| str_field("short_name")) {
            self.short_name = v;
        }
        if let Some(v) = str_field("city") {
            self.city = v;
        }
        if let Some(v) = str_field("organisation").or_else(|| str_field("organization")) {
            self.organisation = v;
        }
        // Host names follow plugin.py `_HOST`; any other value keeps the level above, so a
        // broken block never puts a stray character into a link, a redirect or the CSP.
        if let Some(v) = str_field("orgDomain")
            .or_else(|| str_field("org_domain"))
            .filter(|v| is_host_name(v))
        {
            self.org_domain = v;
        }
        if let Some(v) = str_field("domain").filter(|v| is_host_name(v)) {
            self.domain = v;
        }
        if let Some(v) = str_field("contactEmail").or_else(|| str_field("contact_email")) {
            self.contact_email = v;
        }
        if let Some(v) =
            str_field("documentationBaseUrl").or_else(|| str_field("documentation_base_url"))
        {
            self.documentation_base_url = v;
        }
        if let Some(v) = str_field("licenseDefault").or_else(|| str_field("license_default")) {
            self.license_default = v;
        }
        if let Some(v) = str_field("logo") {
            self.logo = v;
        }
        if let Some(v) = str_field("favicon") {
            self.favicon = v;
        }
        if let Some(v) = str_field("logoSvg").or_else(|| str_field("logo_svg")) {
            self.logo_svg = Some(v);
        }

        if let Some(Value::Object(cols)) = obj.get("colours").or_else(|| obj.get("colors")) {
            if let Some(c) = cols.get("primary").and_then(Value::as_str) {
                if is_valid_hex_colour(c) {
                    self.colours.primary = c.to_owned();
                }
            }
            if let Some(c) = cols.get("secondary").and_then(Value::as_str) {
                if is_valid_hex_colour(c) {
                    self.colours.secondary = c.to_owned();
                }
            }
            if let Some(c) = cols.get("accent").and_then(Value::as_str) {
                if is_valid_hex_colour(c) {
                    self.colours.accent = c.to_owned();
                }
            }
            if let Some(c) = cols.get("background").and_then(Value::as_str) {
                if is_valid_hex_colour(c) {
                    self.colours.background = c.to_owned();
                }
            }
            if let Some(c) = cols.get("text").and_then(Value::as_str) {
                if is_valid_hex_colour(c) {
                    self.colours.text = c.to_owned();
                }
            }
        } else {
            // Also support flat ThemeTokens override (`primary`, `secondary`, `accent`, `background`, `text`)
            if let Some(c) = obj
                .get("primary")
                .and_then(Value::as_str)
                .filter(|c| is_valid_hex_colour(c))
            {
                self.colours.primary = c.to_owned();
            }
            if let Some(c) = obj
                .get("secondary")
                .and_then(Value::as_str)
                .filter(|c| is_valid_hex_colour(c))
            {
                self.colours.secondary = c.to_owned();
            }
            if let Some(c) = obj
                .get("accent")
                .and_then(Value::as_str)
                .filter(|c| is_valid_hex_colour(c))
            {
                self.colours.accent = c.to_owned();
            }
            if let Some(c) = obj
                .get("background")
                .and_then(Value::as_str)
                .filter(|c| is_valid_hex_colour(c))
            {
                self.colours.background = c.to_owned();
            }
            if let Some(c) = obj
                .get("text")
                .and_then(Value::as_str)
                .filter(|c| is_valid_hex_colour(c))
            {
                self.colours.text = c.to_owned();
            }
        }

        if let Some(Value::Object(f)) = obj.get("fonts") {
            if let Some(h) = f
                .get("heading")
                .and_then(Value::as_str)
                .and_then(sanitize_font_stack)
            {
                self.fonts.heading = h;
            }
            if let Some(b) = f
                .get("body")
                .and_then(Value::as_str)
                .and_then(sanitize_font_stack)
            {
                self.fonts.body = b;
            }
        }

        if let Some(Value::Object(l)) = obj.get("languages") {
            if let Some(def) = l
                .get("default")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                self.languages.default = def.to_owned();
            }
            if let Some(Value::Array(arr)) = l.get("offered") {
                let list: Vec<String> = arr
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !list.is_empty() {
                    self.languages.offered = list;
                }
            }
        }

        if let Some(t) = obj.get("tagline") {
            if let Some(s) = t.as_str() {
                self.tagline = s.to_owned();
            }
        }
        if let Some(lines) = obj.get("footerLines").or_else(|| obj.get("footer_lines")) {
            if let Some(arr) = lines.as_array() {
                self.footer_lines = arr
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
        }
        if let Some(r) = obj.get("ribbon") {
            if let Some(s) = r.as_str() {
                self.ribbon = s.trim().to_owned();
            }
        }
        if let Some(ni) = obj.get("noindex") {
            self.noindex = ni.as_bool() == Some(true);
        }

        if let Some(st) = obj.get("siteTitle").or_else(|| obj.get("site_title")) {
            match st {
                Value::String(s) if !s.is_empty() => {
                    self.site_title
                        .insert(self.languages.default.clone(), s.clone());
                }
                Value::Object(map) => {
                    for (k, v) in map {
                        if let Some(s) = v.as_str().filter(|s| !s.is_empty()) {
                            self.site_title.insert(k.clone(), s.to_owned());
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(stl) = obj.get("siteTagline").or_else(|| obj.get("site_tagline")) {
            match stl {
                Value::String(s) if !s.is_empty() => {
                    self.site_tagline
                        .insert(self.languages.default.clone(), s.clone());
                }
                Value::Object(map) => {
                    for (k, v) in map {
                        if let Some(s) = v.as_str().filter(|s| !s.is_empty()) {
                            self.site_tagline.insert(k.clone(), s.to_owned());
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(sl) = str_field("siteLogo").or_else(|| str_field("site_logo")) {
            if is_safe_logo_filename(&sl) {
                self.site_logo = Some(sl);
            }
        }
        if let Some(si) = str_field("siteIssued").or_else(|| str_field("site_issued")) {
            self.site_issued = Some(si);
        }
        if let Some(mcp) = str_field("mcpServer")
            .or_else(|| str_field("mcp_server"))
            .filter(|v| is_mcp_server(v))
        {
            self.mcp_server = Some(mcp);
        }
        // Each address on its own, like `domain`: one that is not a base URL keeps the level above.
        if let Some(Value::Object(addrs)) = obj.get("addresses") {
            for which in Address::ALL {
                if let Some(v) = addrs
                    .get(which.label())
                    .and_then(Value::as_str)
                    .map(|v| v.trim().trim_end_matches('/'))
                    .filter(|v| is_base_url(v))
                {
                    *which.of_mut(&mut self.addresses) = v.to_owned();
                }
            }
        }
        if let Some(Value::Object(samp)) = obj.get("samples") {
            for (lang, arr) in samp {
                if let Some(items) = arr.as_array() {
                    let qs: Vec<String> = items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                    if !qs.is_empty() {
                        self.samples.insert(lang.clone(), qs);
                    }
                }
            }
        }

        self.recompute_derived();
    }

    /// The base of one live part: the address named, else `https://{label}.{domain}` when the
    /// block has a domain, else none.
    #[must_use]
    pub fn address(&self, which: Address) -> Option<String> {
        let named = which.of(&self.addresses);
        if !named.is_empty() {
            return Some(named.to_owned());
        }
        is_host_name(&self.domain).then(|| format!("https://{}.{}", which.label(), self.domain))
    }

    pub fn recompute_derived(&mut self) {
        let (r, g, b) = parse_rgb(&self.colours.primary).unwrap_or((29, 78, 216));
        self.primary_rgb = format!("{r}, {g}, {b}");
        self.primary_foreground = yiq_foreground(r, g, b).into();
    }

    /// Returns the site title in `lang` (`jc_site_title` in `plugin.py`).
    pub fn site_title_in(&self, lang: &str) -> String {
        localized_text(&self.site_title, lang, &self.languages.default)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if !self.instance_name.is_empty() {
                    self.instance_name.clone()
                } else {
                    "joinedcontext".into()
                }
            })
    }

    /// Returns the site tagline in `lang` (`jc_site_tagline` in `plugin.py`), or `None` to fall
    /// back to the localized `tagline` UI string.
    pub fn site_tagline_in(&self, lang: &str) -> Option<String> {
        if let Some(t) = localized_text(&self.site_tagline, lang, &self.languages.default) {
            return Some(t.to_owned());
        }
        if !self.tagline.is_empty() {
            return Some(self.tagline.clone());
        }
        None
    }

    /// Returns the header logo URL (`jc_site_logo` in `plugin.py`).
    pub fn site_logo_path(&self, mount_prefix: &str) -> String {
        let pfx = mount_prefix.trim_end_matches('/');
        if let Some(ref logo) = self.site_logo {
            if logo.starts_with('/') {
                return format!("{pfx}{logo}");
            }
            if is_safe_logo_filename(logo) {
                return format!("{pfx}/{logo}");
            }
        }
        format!("{pfx}/jc-logo.svg")
    }
}

/// Checks `[A-Za-z0-9][A-Za-z0-9._-]*\.(svg|png|webp)` (`jc_site_logo` in `plugin.py`).
pub fn is_safe_logo_filename(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if !matches!(ext, "svg" | "png" | "webp") {
        return false;
    }
    !stem.is_empty()
        && stem.as_bytes()[0].is_ascii_alphanumeric()
        && stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `_localized(values, language)` from `plugin.py` line 456:
/// `values[language]`, else the catalogue's default language, else the first non-empty value (C4).
pub fn localized_text<'a>(
    values: &'a Texts,
    language: &str,
    catalogue_default: &str,
) -> Option<&'a str> {
    if let Some(v) = values.get(language).filter(|s| !s.is_empty()) {
        return Some(v.as_str());
    }
    if let Some(v) = values.get(catalogue_default).filter(|s| !s.is_empty()) {
        return Some(v.as_str());
    }
    text_in_lang(values, Some(language))
}

/// Publisher-level branding entry (`organizations.json` / `Publisher`, B1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublisherBranding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initials: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub description: Texts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
}

/// Generates the publisher's initials badge SVG data URI (`_badge` in `plugin.py` lines 200-213, B1).
pub fn publisher_badge_svg_data_uri(initials: &str, colour: &str) -> Option<String> {
    let trimmed = initials.trim();
    if trimmed.is_empty() || !is_valid_hex_colour(colour) {
        return None;
    }
    let (r, g, b) = parse_rgb(colour)?;
    let ink = yiq_foreground(r, g, b);
    let chars: String = trimmed.chars().take(4).collect();
    let escaped = chars
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"400\" height=\"200\" viewBox=\"0 0 400 200\"><rect width=\"400\" height=\"200\" fill=\"{colour}\"/><text x=\"200\" y=\"100\" dy=\"0.35em\" text-anchor=\"middle\" fill=\"{ink}\" font-family=\"system-ui, sans-serif\" font-size=\"72\" font-weight=\"700\">{escaped}</text></svg>"
    );
    let b64 = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
    Some(format!("data:image/svg+xml;base64,{b64}"))
}

/// Resolves the publisher's image (uploaded logo first, then `_badge` SVG data URI) and localized
/// description (`jc_org` in `plugin.py` lines 216-228, B1, D5).
pub fn resolve_publisher_branding(
    logo_url: Option<&str>,
    descriptions: &Texts,
    fallback: Option<&PublisherBranding>,
    lang: &str,
    default_lang: &str,
) -> (Option<String>, Option<String>) {
    let image = logo_url
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let fb = fallback?;
            if let Some(ref l) = fb.logo_url {
                if !l.is_empty() {
                    return Some(l.clone());
                }
            }
            publisher_badge_svg_data_uri(fb.initials.as_deref()?, fb.colour.as_deref()?)
        });

    let description = localized_text(descriptions, lang, default_lang)
        .map(str::to_owned)
        .or_else(|| {
            let fb = fallback?;
            localized_text(&fb.description, lang, default_lang).map(str::to_owned)
        });

    (image, description)
}

/// Relative luminance of an sRGB colour `(r, g, b)` per WCAG 2.1 §1.4.3.
pub fn relative_luminance(r: u8, g: u8, b: u8) -> f64 {
    let chan = |c: u8| -> f64 {
        let s = f64::from(c) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * chan(r) + 0.7152 * chan(g) + 0.0722 * chan(b)
}

/// WCAG 2.1 contrast ratio between two `#rgb` / `#rrggbb` colours (`1.0 ..= 21.0`, B2).
pub fn wcag_contrast_ratio(hex_a: &str, hex_b: &str) -> Option<f64> {
    let (r1, g1, b1) = parse_rgb(hex_a)?;
    let (r2, g2, b2) = parse_rgb(hex_b)?;
    let l1 = relative_luminance(r1, g1, b1);
    let l2 = relative_luminance(r2, g2, b2);
    let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    Some((lighter + 0.05) / (darker + 0.05))
}

/// Maximum logo upload size in bytes (`1 MiB`, B3).
pub const MAX_LOGO_BYTES: usize = 1024 * 1024;
/// Maximum raster image dimension (`4096 px`, B3).
pub const MAX_IMAGE_DIMENSION: u32 = 4096;

/// Validated, content-addressed logo/image asset stored for a catalogue or publisher (B3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SanitisedAsset {
    pub content_type: String,
    pub extension: String,
    pub sha256_hex: String,
    pub url_path: String,
    pub bytes: Vec<u8>,
}

/// Validates and sanitises an uploaded SVG, PNG, or WebP image (B3):
/// - SVG: must be valid UTF-8 `<svg ...>`, with no `<script>`, no `<foreignObject>`, no `<iframe>`,
///   no `<object>`, no `<embed>`, no `on*=` event attributes (`onload=`, `onclick=`, ...), no
///   `javascript:` or `data:text/html`, and no external `href` / `xlink:href` (`http://`, `https://`, `//`).
/// - PNG / WebP: checks magic bytes, byte size cap (`1 MiB`), and pixel dimension cap (`4096x4096`).
/// - Returns a [`SanitisedAsset`] whose URL path includes a content hash (`/assets/logo-{hash}.{ext}`).
pub fn validate_and_sanitise_logo(bytes: &[u8], content_type: &str) -> Result<SanitisedAsset> {
    if bytes.is_empty() {
        return Err(DataroomError::field_error("logo", "empty image upload"));
    }
    if bytes.len() > MAX_LOGO_BYTES {
        return Err(DataroomError::field_error(
            "logo",
            format!(
                "image size {} exceeds limit of {MAX_LOGO_BYTES} bytes",
                bytes.len()
            ),
        ));
    }

    let ct_clean = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    let (canonical_ct, ext) =
        if ct_clean == "image/svg+xml" || bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml")
        {
            validate_svg_bytes(bytes)?;
            ("image/svg+xml", "svg")
        } else if ct_clean == "image/png" || bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            validate_png_bytes(bytes)?;
            ("image/png", "png")
        } else if ct_clean == "image/webp"
            || (bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
        {
            validate_webp_bytes(bytes)?;
            ("image/webp", "webp")
        } else {
            return Err(DataroomError::field_error(
                "logo",
                "unsupported image format; only SVG, PNG and WebP are allowed",
            ));
        };

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let full_hex = format!("{:x}", hasher.finalize());
    let short_hash = &full_hex[..16];
    let url_path = format!("/assets/logo-{short_hash}.{ext}");

    Ok(SanitisedAsset {
        content_type: canonical_ct.to_owned(),
        extension: ext.to_owned(),
        sha256_hex: full_hex,
        url_path,
        bytes: bytes.to_vec(),
    })
}

fn validate_svg_bytes(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| DataroomError::field_error("logo", "SVG must be valid UTF-8"))?;
    let lower = text.to_ascii_lowercase();

    if !lower.contains("<svg") || !lower.contains("</svg>") {
        return Err(DataroomError::field_error(
            "logo",
            "SVG must contain a root <svg>...</svg> element",
        ));
    }

    // Refuse dangerous elements and URI schemes
    for forbidden in [
        "<script",
        "</script",
        "<foreignobject",
        "<iframe",
        "<object",
        "<embed",
        "javascript:",
        "vbscript:",
        "data:text/html",
        "@import",
        "<!entity",
    ] {
        if lower.contains(forbidden) {
            return Err(DataroomError::field_error(
                "logo",
                format!("SVG contains forbidden construct '{forbidden}'"),
            ));
        }
    }

    // Refuse every CSS `url(...)` that is not a local `#fragment` (style attributes and <style>).
    let mut css = lower.as_str();
    while let Some(pos) = css.find("url(") {
        let after = css[pos + 4..].trim_start().trim_start_matches(['"', '\'']);
        if !after.starts_with('#') {
            return Err(DataroomError::field_error(
                "logo",
                "SVG contains a forbidden external url() reference",
            ));
        }
        css = &css[pos + 4..];
    }

    for tag in svg_tags(&lower) {
        for (attr_name, attr_val) in &tag.attributes {
            check_svg_attribute(attr_name, attr_val)?;
        }
    }
    Ok(())
}

/// One start or end tag of an SVG document: its name and its attributes (values unquoted).
struct SvgTag<'a> {
    attributes: Vec<(&'a str, &'a str)>,
}

/// Splits `text` into tags with their attributes. Quoted values may hold `>` and `/`;
/// comments, processing instructions and declarations are skipped.
fn svg_tags(text: &str) -> Vec<SvgTag<'_>> {
    let bytes = text.as_bytes();
    let mut tags = Vec::new();
    let mut i = 0;
    while let Some(off) = text[i..].find('<') {
        i += off + 1;
        if text[i..].starts_with("!--") {
            match text[i..].find("-->") {
                Some(end) => i += end + 3,
                None => break,
            }
            continue;
        }
        if text[i..].starts_with('?') || text[i..].starts_with('!') {
            match text[i..].find('>') {
                Some(end) => i += end + 1,
                None => break,
            }
            continue;
        }
        if bytes.get(i) == Some(&b'/') {
            i += 1;
        }
        // Tag name.
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'/'
            && bytes[i] != b'>'
        {
            i += 1;
        }
        let mut attributes = Vec::new();
        loop {
            while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
                i += 1;
            }
            if i >= bytes.len() || bytes[i] == b'>' {
                i += 1;
                break;
            }
            let name_start = i;
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && !matches!(bytes[i], b'=' | b'>' | b'/')
            {
                i += 1;
            }
            let name = &text[name_start..i];
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let mut value = "";
            if bytes.get(i) == Some(&b'=') {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                match bytes.get(i) {
                    Some(&q) if q == b'"' || q == b'\'' => {
                        let value_start = i + 1;
                        let value_end = text[value_start..]
                            .find(q as char)
                            .map_or(bytes.len(), |e| value_start + e);
                        value = &text[value_start..value_end];
                        i = value_end + 1;
                    }
                    _ => {
                        let value_start = i;
                        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>'
                        {
                            i += 1;
                        }
                        value = &text[value_start..i];
                    }
                }
            }
            if name.is_empty() {
                i += 1;
                continue;
            }
            attributes.push((name, value));
        }
        tags.push(SvgTag { attributes });
        if i >= bytes.len() {
            break;
        }
    }
    tags
}

/// No event handler, no reference that leaves the document: `href`, `xlink:href` and `src` may
/// only name a local `#fragment` or an embedded raster image; animation may not retarget them.
fn check_svg_attribute(name: &str, value: &str) -> Result<()> {
    let refuse = |why: String| Err(DataroomError::field_error("logo", why));
    if name.starts_with("on") && name.len() > 2 {
        return refuse(format!(
            "SVG contains forbidden event handler attribute '{name}'"
        ));
    }
    let value = value.trim();
    if matches!(name, "href" | "src") || name.ends_with(":href") {
        let local = value.starts_with('#');
        let embedded_raster = [
            "data:image/png",
            "data:image/jpeg",
            "data:image/gif",
            "data:image/webp",
        ]
        .iter()
        .any(|p| value.starts_with(p));
        if !local && !embedded_raster {
            return refuse(format!(
                "SVG contains forbidden external reference in '{name}'"
            ));
        }
    }
    if name == "attributename"
        && (value == "href" || value.ends_with(":href") || value.starts_with("on"))
    {
        return refuse(format!("SVG animation may not set '{value}'"));
    }
    Ok(())
}

fn validate_png_bytes(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 24 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return Err(DataroomError::field_error("logo", "invalid PNG header"));
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(DataroomError::field_error(
            "logo",
            format!("PNG dimensions {width}x{height} exceed limit of {MAX_IMAGE_DIMENSION}px"),
        ));
    }
    Ok(())
}

fn validate_webp_bytes(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 30 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(DataroomError::field_error("logo", "invalid WebP header"));
    }
    // The canvas size sits in the first chunk (RFC 9649): VP8X stores width-1 and height-1 in
    // 24 bits each, a lossless VP8L bitstream 14 bits each after its 0x2f signature, a lossy VP8
    // frame 14 bits each after its start code.
    let le24 = |i: usize| {
        u32::from(bytes[i]) | u32::from(bytes[i + 1]) << 8 | u32::from(bytes[i + 2]) << 16
    };
    let (width, height) = match &bytes[12..16] {
        b"VP8X" => (le24(24) + 1, le24(27) + 1),
        b"VP8L" if bytes[20] == 0x2f => {
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
        }
        b"VP8 " if bytes[23..26] == [0x9d, 0x01, 0x2a] => (
            u32::from(u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3fff),
            u32::from(u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3fff),
        ),
        _ => return Err(DataroomError::field_error("logo", "invalid WebP header")),
    };
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(DataroomError::field_error(
            "logo",
            format!("WebP dimensions {width}x{height} exceed limit of {MAX_IMAGE_DIMENSION}px"),
        ));
    }
    Ok(())
}

/// plugin.py `_HOST`: dot-separated lowercase labels of `[a-z0-9-]`, no label starting or ending
/// with `-`, at least two labels.
pub fn is_host_name(v: &str) -> bool {
    let labels: Vec<&str> = v.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// plugin.py `MCP_SERVER`: `d/` and a public id of 1..63 `[-a-z0-9]`, not starting or ending with `-`.
pub fn is_mcp_server(v: &str) -> bool {
    let Some(id) = v.strip_prefix("d/") else {
        return false;
    };
    (1..=63).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn b1_three_level_branding_overrides_fallbacks_and_loads_dev_stage_smartcity() {
        // 1. Load dev, stage and smartcity blocks from the pack
        let dev_yaml = include_str!("../tests/fixtures/branding/dev-branding.yaml");
        let stage_yaml = include_str!("../tests/fixtures/branding/stage-branding.yaml");
        let smartcity_yaml = include_str!("../tests/fixtures/branding/smartcity-branding.yaml");

        let dev = BrandingBlock::from_yaml_lossy(dev_yaml);
        assert_eq!(dev.instance_name, "joinedcontext");
        assert_eq!(dev.city, "Helsinki");
        assert_eq!(dev.org_domain, "hel.fi");
        assert_eq!(dev.domain, "dev.joinedcontext.com");
        assert_eq!(dev.contact_email, "hello@joinedcontext.com");
        assert_eq!(
            dev.documentation_base_url,
            "https://dev.joinedcontext.com/docs"
        );
        assert_eq!(dev.colours.primary, "#0000bf");
        assert_eq!(dev.primary_rgb, "0, 0, 191");
        assert_eq!(dev.primary_foreground, "#ffffff");

        let stage = BrandingBlock::from_yaml_lossy(stage_yaml);
        assert_eq!(stage.domain, "stage.joinedcontext.com");
        assert_eq!(
            stage.documentation_base_url,
            "https://stage.joinedcontext.com/docs"
        );

        let smartcity = BrandingBlock::from_yaml_lossy(smartcity_yaml);
        assert_eq!(smartcity.domain, "smartcity.joinedcontext.com");
        assert_eq!(
            smartcity.documentation_base_url,
            "https://smartcity.joinedcontext.com/docs"
        );

        // 2. Broken / non-dict JSON falls back to NEUTRAL without erroring
        let broken = BrandingBlock::from_str_lossy("not valid json {{{");
        assert_eq!(broken.instance_name, "joinedcontext");
        assert_eq!(broken.colours.primary, DEFAULT_PRIMARY);

        // 3. Three levels: installation -> catalogue -> publisher, later wins, bad colour ignored
        let mut effective = dev.clone();
        effective.apply_override_json(&json!({
            "instanceName": "Banská Bystrica Open Data",
            "colours": {
                "primary": "#ffe977",
                "secondary": "javascript:alert(1)" // bad colour -> ignored!
            },
            "siteTitle": {
                "sk": "Otvorené dáta BB",
                "en": "BB Open Data"
            },
            "ribbon": "  Demo catalogue  ",
            "noindex": true
        }));
        assert_eq!(effective.instance_name, "Banská Bystrica Open Data");
        // #ffe977 is a light brand colour -> YIQ foreground must be #000000!
        assert_eq!(effective.colours.primary, "#ffe977");
        assert_eq!(effective.primary_foreground, "#000000");
        assert_eq!(effective.primary_rgb, "255, 233, 119");
        // Bad secondary colour was ignored and kept dev's #0072c6:
        assert_eq!(effective.colours.secondary, "#0072c6");
        assert_eq!(effective.ribbon, "Demo catalogue");
        assert!(effective.noindex);
        assert_eq!(effective.site_title_in("sk"), "Otvorené dáta BB");
        assert_eq!(effective.site_title_in("en"), "BB Open Data");

        // Publisher level: badge fallback vs custom logo override
        let pub_fallback = PublisherBranding {
            initials: Some("BB".into()),
            colour: Some("#00579c".into()),
            logo_url: None,
            description: BTreeMap::from([
                ("sk".into(), "Otvorené údaje mesta Banská Bystrica.".into()),
                (
                    "en".into(),
                    "Open data of the City of Banská Bystrica.".into(),
                ),
            ]),
            homepage: Some("https://www.banskabystrica.sk".into()),
        };
        let (badge_img, desc_sk) =
            resolve_publisher_branding(None, &BTreeMap::new(), Some(&pub_fallback), "sk", "sk");
        assert!(badge_img.unwrap().starts_with("data:image/svg+xml;base64,"));
        assert_eq!(
            desc_sk.as_deref(),
            Some("Otvorené údaje mesta Banská Bystrica.")
        );

        let own_desc = BTreeMap::from([("sk".into(), "Vlastný popis vydavateľa.".into())]);
        let (custom_img, custom_desc) = resolve_publisher_branding(
            Some("/assets/logo-custom.svg"),
            &own_desc,
            Some(&pub_fallback),
            "sk",
            "sk",
        );
        assert_eq!(custom_img.as_deref(), Some("/assets/logo-custom.svg"));
        assert_eq!(custom_desc.as_deref(), Some("Vlastný popis vydavateľa."));
    }

    #[test]
    fn b2_every_look_of_every_branding_reads_at_wcag_aa() {
        for yaml in [
            include_str!("../tests/fixtures/branding/dev-branding.yaml"),
            include_str!("../tests/fixtures/branding/stage-branding.yaml"),
            include_str!("../tests/fixtures/branding/smartcity-branding.yaml"),
        ] {
            let b = BrandingBlock::from_yaml_lossy(yaml);
            // 1. text on background >= 4.5:1 (WCAG AA normal text)
            let text_bg = wcag_contrast_ratio(&b.colours.text, &b.colours.background).unwrap();
            assert!(
                text_bg >= 4.5,
                "text {} on background {} has contrast {text_bg:.2} < 4.5",
                b.colours.text,
                b.colours.background
            );
            // 2. primaryForeground on primary >= 4.5:1
            let fg_primary =
                wcag_contrast_ratio(&b.primary_foreground, &b.colours.primary).unwrap();
            assert!(
                fg_primary >= 4.5,
                "primaryForeground {} on primary {} has contrast {fg_primary:.2} < 4.5",
                b.primary_foreground,
                b.colours.primary
            );
            // 3. links (primary) on background >= 4.5:1
            let link_bg = wcag_contrast_ratio(&b.colours.primary, &b.colours.background).unwrap();
            assert!(
                link_bg >= 4.5,
                "primary link {} on background {} has contrast {link_bg:.2} < 4.5",
                b.colours.primary,
                b.colours.background
            );
        }

        // Also check neutral default block and a light primary colour (#ffe977)
        let neutral = BrandingBlock::default();
        assert!(
            wcag_contrast_ratio(&neutral.colours.text, &neutral.colours.background).unwrap() >= 4.5
        );
        assert!(
            wcag_contrast_ratio(&neutral.primary_foreground, &neutral.colours.primary).unwrap()
                >= 4.5
        );
        assert!(
            wcag_contrast_ratio(&neutral.colours.primary, &neutral.colours.background).unwrap()
                >= 4.5
        );
    }

    #[test]
    fn b1_broken_domain_and_mcp_server_fall_back_to_the_level_above() {
        let mut block = BrandingBlock::default();
        block.apply_override_json(
            &json!({ "domain": "dev.joinedcontext.com", "mcpServer": "d/demo-1" }),
        );
        assert_eq!(block.domain, "dev.joinedcontext.com");
        assert_eq!(block.mcp_server.as_deref(), Some("d/demo-1"));
        for bad in [
            "x; script-src *",
            "Dev.Example.com",
            "localhost",
            "-a.example",
            "a..example",
            "evil.example/path",
        ] {
            let mut b = block.clone();
            b.apply_override_json(&json!({ "domain": bad, "orgDomain": bad }));
            assert_eq!(
                b.domain, "dev.joinedcontext.com",
                "domain {bad:?} must be ignored"
            );
        }
        for bad in ["d/", "d/-x", "d/<script>", "e/demo", "d/a/b"] {
            let mut b = block.clone();
            b.apply_override_json(&json!({ "mcpServer": bad }));
            assert_eq!(
                b.mcp_server.as_deref(),
                Some("d/demo-1"),
                "mcpServer {bad:?} must be ignored"
            );
        }
    }

    #[test]
    fn g1_each_address_is_named_on_its_own_or_follows_the_domain() {
        let mut block = BrandingBlock::default();
        for which in Address::ALL {
            assert_eq!(block.address(which), None, "no domain, no {which:?}");
        }
        block.apply_override_json(&json!({ "addresses": {
            "viz": "http://localhost:42241/viz/",
            "api": "https://api.city.example",
            "tiles": "https://tiles.example:8443",
        }}));
        assert_eq!(
            block.address(Address::Viz).as_deref(),
            Some("http://localhost:42241/viz")
        );
        assert_eq!(
            block.address(Address::Api).as_deref(),
            Some("https://api.city.example")
        );
        assert_eq!(
            block.address(Address::Tiles).as_deref(),
            Some("https://tiles.example:8443")
        );
        assert_eq!(
            block.address(Address::Assistant),
            None,
            "an assistant nobody named stays empty"
        );
        assert_eq!(block.address(Address::Portal), None);

        let mut dom = BrandingBlock::default();
        dom.apply_override_json(&json!({ "domain": "dev.joinedcontext.com" }));
        assert_eq!(
            dom.address(Address::Assistant).as_deref(),
            Some("https://assistant.dev.joinedcontext.com")
        );
        dom.apply_override_json(&json!({ "addresses": { "api": "https://gw.example" } }));
        assert_eq!(
            dom.address(Address::Api).as_deref(),
            Some("https://gw.example"),
            "named wins"
        );

        for bad in [
            "javascript:alert(1)",
            "https://x.example/a?b",
            "https://x.example/#a",
            "https://user@x.example",
            "https://x.example/a/../b",
            "https://x.example/a b",
            "https://x.example\"; script-src *",
            "https://X.example",
            "ftp://x.example",
            "https://x.example:99999999",
            "//x.example",
        ] {
            let mut b = block.clone();
            b.apply_override_json(&json!({ "addresses": { "api": bad } }));
            assert_eq!(
                b.addresses.api, "https://api.city.example",
                "{bad:?} must be ignored"
            );
        }
        assert_eq!(
            origin_of("http://localhost:42241/viz"),
            "http://localhost:42241"
        );
        assert_eq!(origin_of("https://api.example"), "https://api.example");
    }

    #[test]
    fn b3_svg_with_script_onload_and_external_image_href_is_refused() {
        // 1. Valid clean SVG passes and gets a content hash URL
        let clean_svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 30"><rect width="100" height="30" fill="#0000bf"/></svg>"##;
        let asset =
            validate_and_sanitise_logo(clean_svg, "image/svg+xml").expect("clean SVG passes");
        assert!(asset.url_path.starts_with("/assets/logo-"));
        assert!(asset.url_path.ends_with(".svg"));
        assert_eq!(asset.content_type, "image/svg+xml");

        // 2. SVG with <script> is refused
        let script_svg =
            br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
        assert!(validate_and_sanitise_logo(script_svg, "image/svg+xml").is_err());

        // 3. SVG with onload= event attribute is refused
        let onload_svg =
            br#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"><rect width="10" height="10"/></svg>"#;
        assert!(validate_and_sanitise_logo(onload_svg, "image/svg+xml").is_err());

        // 4. SVG with external <image href="https://..."> is refused
        let ext_href_svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="https://evil.example/track.png" width="10" height="10"/></svg>"#;
        assert!(validate_and_sanitise_logo(ext_href_svg, "image/svg+xml").is_err());

        let xlink_href_svg = br#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><image xlink:href="http://evil.example/track.png"/></svg>"#;
        assert!(validate_and_sanitise_logo(xlink_href_svg, "image/svg+xml").is_err());

        // 5. Spacing, quoting and CSS do not slip an external reference or a handler past it.
        for svg in [
            &br#"<svg xmlns="http://www.w3.org/2000/svg"><image href = 'https://evil.example/t.png'/></svg>"#[..],
            &br#"<svg xmlns="http://www.w3.org/2000/svg"><image href=//evil.example/t.png /></svg>"#[..],
            &br#"<svg xmlns="http://www.w3.org/2000/svg"><use href="other.svg#a"/></svg>"#[..],
            &br#"<svg xmlns="http://www.w3.org/2000/svg"><rect title="a>b" ONLOAD="x()"/></svg>"#[..],
            &br#"<svg xmlns="http://www.w3.org/2000/svg"><rect style="fill:url(https://evil.example/p)"/></svg>"#[..],
            &br#"<svg xmlns="http://www.w3.org/2000/svg"><a href="&#106;avascript:x()"><rect/></a></svg>"#[..],
            &br#"<svg xmlns="http://www.w3.org/2000/svg"><a><set attributeName="href" to="https://evil.example"/></a></svg>"#[..],
        ] {
            assert!(
                validate_and_sanitise_logo(svg, "image/svg+xml").is_err(),
                "must refuse {}",
                String::from_utf8_lossy(svg)
            );
        }

        // 6. Local references and gradients stay allowed.
        let local_svg = br##"<svg xmlns="http://www.w3.org/2000/svg"><defs><linearGradient id="g"/></defs><rect fill="url(#g)"/><use href="#g"/></svg>"##;
        assert!(validate_and_sanitise_logo(local_svg, "image/svg+xml").is_ok());
    }
}
