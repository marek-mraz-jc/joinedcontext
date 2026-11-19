//! `Organization.spec.appearance`: what an Organization's Portal, open data portal and login page
//! show of it (UIX-16, T-6772).
//!
//! Every member is optional and overrides the installation's branding file field by field; a
//! member left out shows the installation's value. Each one is a validated value, never raw CSS
//! or HTML: a colour is a hex triplet or sextet, a name is one line of plain text, a logo is an
//! SVG without script, event handler, external reference or entity.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The longest name, city or publisher an appearance carries.
pub const MAX_APPEARANCE_TEXT: usize = 120;

/// The largest logo or favicon SVG an appearance carries inline, in bytes.
pub const MAX_APPEARANCE_SVG: usize = 64 * 1024;

/// The most Dashboards an Organization's Portal home page shows.
pub const MAX_HOME_WIDGETS: usize = 6;

/// `spec.appearance` (UIX-16).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OrganizationAppearance {
    /// Full name on page titles and the login page, e.g. `Mesto Banská Bystrica`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_name: Option<String>,
    /// Short name on sidebars and tabs, e.g. `BB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_name: Option<String>,
    /// The city or region the Organization serves, e.g. `Banská Bystrica`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// The legal publisher named in the footer and the open data portal, e.g. `Mesto Banská Bystrica`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// The contact address in the footer and the open data portal, e.g. `opendata@banskabystrica.sk`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_email: Option<String>,
    /// The licence a new dataset starts with, e.g. `CC-BY-4.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_default: Option<String>,
    /// The colour tokens, each a hex colour such as `#0b5394`.
    #[serde(default, skip_serializing_if = "AppearanceColours::is_unset")]
    pub colours: AppearanceColours,
    /// The logo as an SVG document of at most 64 KiB, without script, event handler or external reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_svg: Option<String>,
    /// The favicon as an SVG document of at most 64 KiB, under the same rules as the logo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon_svg: Option<String>,
    /// Dashboards the Organization's Portal home page shows, each `project/name` of one of its
    /// projects, e.g. `doprava/prehlad`; a viewer sees only those they may read (T-6779).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub home_widgets: Vec<String>,
    /// Dashboards the Organization's open data portal shows on its catalogue home, each
    /// `project/name` of one of its projects with `visibility: public`, e.g. `doprava/prehlad`;
    /// anonymous readers see them (T-6780).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opendata_widgets: Vec<String>,
    /// The texts of the open data portal's own pages, each one page in one language in
    /// Markdown (T-7031); a page or language left out shows the built-in text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<AppearancePage>,
    /// The open data portal's typeface, one of a fixed set of system font stacks, so no font is
    /// fetched from another host: `system`, `humanist`, `serif` or `mono` (T-7410).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<AppearanceFont>,
    /// The corner radius of cards, buttons and fields in pixels, 0 to 24, e.g. `8`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<u8>,
    /// The open data portal's home hero: its title and an introduction in Markdown.
    #[serde(default, skip_serializing_if = "AppearanceHero::is_unset")]
    pub hero: AppearanceHero,
    /// How the open data portal lists datasets: `cards` (the default) or a compact `table`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset_list: Option<DatasetListLayout>,
    /// Links the open data portal's header adds after its own, at most 8, each a label and an
    /// `https://` address or a path on the portal, e.g. `{label: Mesto, url: https://mesto.sk}`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nav_links: Vec<AppearanceLink>,
    /// Links the open data portal's footer adds after its own, under the same rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub footer_links: Vec<AppearanceLink>,
}

/// The most header or footer links an appearance adds.
pub const MAX_APPEARANCE_LINKS: usize = 8;

/// The longest hero introduction, in bytes of Markdown.
pub const MAX_HERO_INTRO: usize = 2048;

/// The largest corner radius in pixels.
pub const MAX_APPEARANCE_RADIUS: u8 = 24;

/// The least WCAG 2.1 contrast an appearance's text and link colours keep on its background
/// (level AA for body text).
pub const MIN_CONTRAST: f64 = 4.5;

/// `spec.appearance.font`: a system font stack, never a web font (T-7410).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceFont {
    /// The platform's interface font.
    System,
    /// A humanist sans serif.
    Humanist,
    /// A book serif.
    Serif,
    /// A monospace font.
    Mono,
}

impl AppearanceFont {
    /// The CSS font stack, fonts every platform carries or falls back from.
    pub fn stack(self) -> &'static str {
        match self {
            Self::System => "system-ui, -apple-system, \"Segoe UI\", Roboto, sans-serif",
            Self::Humanist => {
                "Seravek, \"Gill Sans Nova\", Ubuntu, Calibri, \"DejaVu Sans\", sans-serif"
            }
            Self::Serif => "Charter, \"Bitstream Charter\", \"Sitka Text\", Cambria, Georgia, serif",
            Self::Mono => "ui-monospace, \"Cascadia Code\", Menlo, Consolas, monospace",
        }
    }
}

/// `spec.appearance.datasetList`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DatasetListLayout {
    /// One card per dataset with its description.
    Cards,
    /// One table row per dataset.
    Table,
}

/// `spec.appearance.hero`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AppearanceHero {
    /// The home page's heading, one line, e.g. `Otvorené dáta Banskej Bystrice`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The introduction under it in Markdown of at most 2 KiB, e.g.
    /// `Dáta mesta na **jednom mieste**.`; raw HTML shows as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
}

impl AppearanceHero {
    /// Whether neither member is set.
    pub fn is_unset(&self) -> bool {
        *self == Self::default()
    }
}

/// `spec.appearance.navLinks[]` and `footerLinks[]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AppearanceLink {
    /// The text of the link, e.g. `Mesto`.
    pub label: String,
    /// An `https://` or `http://` address or a path on the portal starting `/`, e.g.
    /// `https://www.banskabystrica.sk`.
    pub url: String,
}

/// The most page texts an appearance carries: every page in eight languages.
pub const MAX_APPEARANCE_PAGES: usize = 40;

/// `spec.appearance.pages[]`: one page of the open data portal in one language (T-7031).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AppearancePage {
    /// The page: `connector`, `privacy`, `terms`, `support` or `accessibility`.
    pub page: String,
    /// The language as a two-letter code, e.g. `sk`.
    pub lang: String,
    /// The page in Markdown, its first `# heading` the title, at most 16 KiB, e.g.
    /// `# Podpora\n\nNapíšte nám na opendata@mesto.sk.`; raw HTML shows as text.
    pub markdown: String,
}

/// `spec.appearance.colours`: each one a hex colour, `#rgb` or `#rrggbb`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AppearanceColours {
    /// Primary action colour, e.g. `#0b5394`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    /// Secondary colour for links and focus, e.g. `#0f766e`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary: Option<String>,
    /// Highlight colour, e.g. `#f59e0b`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// Page background, e.g. `#ffffff`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Body text, e.g. `#0f172a`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl AppearanceColours {
    /// Whether no colour is set.
    pub fn is_unset(&self) -> bool {
        *self == Self::default()
    }

    fn named(&self) -> [(&'static str, &Option<String>); 5] {
        [
            ("spec.appearance.colours.primary", &self.primary),
            ("spec.appearance.colours.secondary", &self.secondary),
            ("spec.appearance.colours.accent", &self.accent),
            ("spec.appearance.colours.background", &self.background),
            ("spec.appearance.colours.text", &self.text),
        ]
    }
}

impl OrganizationAppearance {
    /// Whether nothing is set, so the installation's branding shows unchanged.
    pub fn is_unset(&self) -> bool {
        *self == Self::default()
    }

    /// Refuses every member that is not the value its field names, with the member's path.
    pub fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("spec.appearance.instanceName", &self.instance_name),
            ("spec.appearance.shortName", &self.short_name),
            ("spec.appearance.city", &self.city),
            ("spec.appearance.publisher", &self.publisher),
        ] {
            if let Some(text) = value {
                plain_text(field, text)?;
            }
        }
        if let Some(email) = &self.contact_email {
            let (local, domain) = email.split_once('@').unwrap_or_default();
            if local.is_empty()
                || !domain.contains('.')
                || domain.starts_with('.')
                || domain.ends_with('.')
                || email.len() > 254
                || domain.contains('@')
                || email
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || "<>\"'".contains(c))
            {
                return Err(invalid(
                    "spec.appearance.contactEmail",
                    "an e-mail address such as opendata@city.example",
                ));
            }
        }
        if let Some(licence) = &self.license_default {
            if licence.is_empty()
                || licence.len() > 64
                || !licence
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-.+".contains(&b))
            {
                return Err(invalid(
                    "spec.appearance.licenseDefault",
                    "an SPDX licence identifier such as CC-BY-4.0",
                ));
            }
        }
        for (field, value) in self.colours.named() {
            if let Some(colour) = value {
                if !is_hex_colour(colour) {
                    return Err(invalid(field, "a hex colour such as #0b5394"));
                }
            }
        }
        for (field, value) in [
            ("spec.appearance.logoSvg", &self.logo_svg),
            ("spec.appearance.faviconSvg", &self.favicon_svg),
        ] {
            if let Some(svg) = value {
                safe_svg(field, svg)?;
            }
        }
        dashboards("spec.appearance.homeWidgets", &self.home_widgets)?;
        dashboards("spec.appearance.opendataWidgets", &self.opendata_widgets)?;
        pages(&self.pages)?;
        self.contrast()?;
        if self.radius.is_some_and(|r| r > MAX_APPEARANCE_RADIUS) {
            return Err(invalid("spec.appearance.radius", "0 to 24 pixels"));
        }
        if let Some(title) = &self.hero.title {
            plain_text("spec.appearance.hero.title", title)?;
        }
        if let Some(intro) = &self.hero.intro {
            if intro.trim().is_empty()
                || intro.len() > MAX_HERO_INTRO
                || intro
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            {
                return Err(invalid(
                    "spec.appearance.hero.intro",
                    "Markdown of 1 to 2048 bytes without control characters",
                ));
            }
        }
        links("spec.appearance.navLinks", &self.nav_links)?;
        links("spec.appearance.footerLinks", &self.footer_links)
    }

    /// Refuses text or primary colour below [`MIN_CONTRAST`] on the background when the
    /// appearance sets both of the pair; a pair with one member set is checked where it meets
    /// the installation's colour, by the page that draws it.
    fn contrast(&self) -> Result<()> {
        let Some(background) = &self.colours.background else {
            return Ok(());
        };
        for (field, colour) in [
            ("spec.appearance.colours.text", &self.colours.text),
            ("spec.appearance.colours.primary", &self.colours.primary),
        ] {
            let ratio = colour
                .as_deref()
                .and_then(|c| contrast_ratio(c, background));
            if ratio.is_some_and(|r| r < MIN_CONTRAST) {
                return Err(invalid(
                    field,
                    "a colour with a contrast of at least 4.5:1 on the background (WCAG AA)",
                ));
            }
        }
        Ok(())
    }

    /// `self` over `under` member by member: what `self` leaves out is `under`'s, as a
    /// `kind: Portal`'s `spec.appearance` goes over its Organization's (T-7410).
    pub fn over(&self, under: &Self) -> Self {
        fn pick<T: Clone>(top: &Option<T>, below: &Option<T>) -> Option<T> {
            top.clone().or_else(|| below.clone())
        }
        fn list<T: Clone>(top: &[T], below: &[T]) -> Vec<T> {
            if top.is_empty() { below } else { top }.to_vec()
        }
        let (c, u) = (&self.colours, &under.colours);
        Self {
            instance_name: pick(&self.instance_name, &under.instance_name),
            short_name: pick(&self.short_name, &under.short_name),
            city: pick(&self.city, &under.city),
            publisher: pick(&self.publisher, &under.publisher),
            contact_email: pick(&self.contact_email, &under.contact_email),
            license_default: pick(&self.license_default, &under.license_default),
            colours: AppearanceColours {
                primary: pick(&c.primary, &u.primary),
                secondary: pick(&c.secondary, &u.secondary),
                accent: pick(&c.accent, &u.accent),
                background: pick(&c.background, &u.background),
                text: pick(&c.text, &u.text),
            },
            logo_svg: pick(&self.logo_svg, &under.logo_svg),
            favicon_svg: pick(&self.favicon_svg, &under.favicon_svg),
            home_widgets: list(&self.home_widgets, &under.home_widgets),
            opendata_widgets: list(&self.opendata_widgets, &under.opendata_widgets),
            pages: list(&self.pages, &under.pages),
            font: self.font.or(under.font),
            radius: self.radius.or(under.radius),
            hero: AppearanceHero {
                title: pick(&self.hero.title, &under.hero.title),
                intro: pick(&self.hero.intro, &under.hero.intro),
            },
            dataset_list: self.dataset_list.or(under.dataset_list),
            nav_links: list(&self.nav_links, &under.nav_links),
            footer_links: list(&self.footer_links, &under.footer_links),
        }
    }
}

/// WCAG 2.1 contrast ratio of two hex colours, 1 to 21; `None` when either is not one.
pub fn contrast_ratio(a: &str, b: &str) -> Option<f64> {
    fn luminance(hex: &str) -> Option<f64> {
        if !is_hex_colour(hex) {
            return None;
        }
        let hex = &hex[1..];
        let channel = |i: usize| -> Option<f64> {
            let v = if hex.len() == 3 {
                u8::from_str_radix(&hex[i..=i].repeat(2), 16).ok()?
            } else {
                u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?
            };
            let s = f64::from(v) / 255.0;
            Some(if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            })
        };
        Some(0.2126 * channel(0)? + 0.7152 * channel(1)? + 0.0722 * channel(2)?)
    }
    let (la, lb) = (luminance(a)?, luminance(b)?);
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    Some((hi + 0.05) / (lo + 0.05))
}

/// Each link has a label and an address a reader may follow: `http(s)://host...` or a path on
/// the portal, never another scheme (`javascript:`, `data:`) or a protocol-relative `//host`.
fn links(member: &str, links: &[AppearanceLink]) -> Result<()> {
    for (at, link) in links.iter().enumerate() {
        let field = format!("{member}[{at}]");
        if at >= MAX_APPEARANCE_LINKS {
            return Err(invalid(&field, "one of at most 8 links"));
        }
        plain_text(&format!("{field}.label"), &link.label)?;
        if !is_link_url(&link.url) {
            return Err(invalid(
                &format!("{field}.url"),
                "an https:// address or a path starting / such as /about",
            ));
        }
    }
    Ok(())
}

/// `https://host...`, `http://host...` or `/path...`, at most 512 bytes of printable ASCII
/// without quotes, angle brackets or backslashes, so it sits in an `href` as it is.
pub fn is_link_url(url: &str) -> bool {
    if url.is_empty()
        || url.len() > 512
        || !url
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"\"'<>\\`".contains(&b))
    {
        return false;
    }
    let host = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    match host {
        Some(rest) => rest
            .split(['/', '?', '#'])
            .next()
            .is_some_and(|h| !h.is_empty() && !h.contains('@')),
        None => url.starts_with('/') && !url.starts_with("//"),
    }
}

/// Each page text names one of the portal's pages and a language, once, with Markdown of 1 byte
/// to [`crate::opendata_pages::MAX_PAGE_MARKDOWN`] and no control character but line breaks
/// and tabs.
fn pages(pages: &[AppearancePage]) -> Result<()> {
    for (at, text) in pages.iter().enumerate() {
        let field = format!("spec.appearance.pages[{at}]");
        if at >= MAX_APPEARANCE_PAGES {
            return Err(invalid(&field, "one of at most 40 page texts"));
        }
        if !crate::opendata_pages::PAGES.contains(&text.page.as_str()) {
            return Err(invalid(
                &format!("{field}.page"),
                "connector, privacy, terms, support or accessibility",
            ));
        }
        if text.lang.len() != 2 || !text.lang.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(invalid(
                &format!("{field}.lang"),
                "a two-letter language code such as sk",
            ));
        }
        if pages[..at]
            .iter()
            .any(|o| o.page == text.page && o.lang == text.lang)
        {
            return Err(invalid(&field, "the only text of its page and language"));
        }
        if text.markdown.trim().is_empty()
            || text.markdown.len() > crate::opendata_pages::MAX_PAGE_MARKDOWN
            || text
                .markdown
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(invalid(
                &format!("{field}.markdown"),
                "Markdown of 1 to 16384 bytes without control characters",
            ));
        }
    }
    Ok(())
}

/// Each of `widgets` names a Dashboard as `project/name`, once, at most [`MAX_HOME_WIDGETS`].
fn dashboards(member: &str, widgets: &[String]) -> Result<()> {
    for (at, widget) in widgets.iter().enumerate() {
        let field = format!("{member}[{at}]");
        let named = widget.split_once('/').is_some_and(|(project, name)| {
            crate::names::validate_dns1123_label(project).is_ok()
                && crate::names::validate_dns1123_label(name).is_ok()
        });
        if !named {
            return Err(invalid(
                &field,
                "a Dashboard as project/name, e.g. doprava/prehlad",
            ));
        }
        if at >= MAX_HOME_WIDGETS {
            return Err(invalid(&field, "one of at most 6 Dashboards"));
        }
        if widgets[..at].contains(widget) {
            return Err(invalid(&field, "a Dashboard not named twice"));
        }
    }
    Ok(())
}

fn invalid(field: &str, reason: &str) -> Error {
    Error::Invalid {
        field: field.to_owned(),
        reason: format!("must be {reason}"),
    }
}

fn plain_text(field: &str, text: &str) -> Result<()> {
    if text.trim().is_empty()
        || text.chars().count() > MAX_APPEARANCE_TEXT
        || text.chars().any(|c| c.is_control() || c == '<' || c == '>')
    {
        return Err(invalid(
            field,
            "one line of 1 to 120 characters without < or >",
        ));
    }
    Ok(())
}

/// `#rgb` or `#rrggbb`: the only colour syntax a token takes, so nothing else reaches a CSS
/// custom property.
pub fn is_hex_colour(value: &str) -> bool {
    value
        .strip_prefix('#')
        .is_some_and(|hex| matches!(hex.len(), 3 | 6) && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// An SVG with a root element and none of the constructs that run script or reach out: script,
/// foreign objects, frames, event handlers, `javascript:`, entities, and any reference or
/// `url()` that is not a local `#fragment`. The Portal also serves it as an attachment with
/// `nosniff`, so a check missed here still renders nothing as a page.
fn safe_svg(field: &str, svg: &str) -> Result<()> {
    let refuse = |what: &str| {
        Err(Error::Invalid {
            field: field.to_owned(),
            reason: format!("must be an SVG of at most 64 KiB without script, event handlers or external references ({what})"),
        })
    };
    if svg.len() > MAX_APPEARANCE_SVG {
        return refuse("too large");
    }
    let lower = svg.to_ascii_lowercase();
    if !lower.contains("<svg") || !lower.trim_end().ends_with("</svg>") {
        return refuse("no <svg> root");
    }
    for forbidden in [
        "<script",
        "<foreignobject",
        "<iframe",
        "<object",
        "<embed",
        "<!entity",
        "<!doctype",
        "javascript:",
        "vbscript:",
        "data:",
        "@import",
    ] {
        if lower.contains(forbidden) {
            return refuse(forbidden);
        }
    }
    let bytes = lower.as_bytes();
    for (at, _) in lower.match_indices("on") {
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let name_end = lower[at + 2..]
            .find(|c: char| !c.is_ascii_alphabetic())
            .map(|n| at + 2 + n);
        let handler = before.is_some_and(|b| b.is_ascii_whitespace() || b == b'/')
            && name_end
                .is_some_and(|end| end > at + 2 && lower[end..].trim_start().starts_with('='));
        if handler {
            return refuse("event handler");
        }
    }
    for attribute in ["href", "src"] {
        for (at, _) in lower.match_indices(attribute) {
            let rest = lower[at + attribute.len()..].trim_start();
            if let Some(value) = rest.strip_prefix('=') {
                let value = value.trim_start().trim_start_matches(['"', '\'']);
                if !value.starts_with('#') {
                    return refuse("external reference");
                }
            }
        }
    }
    let mut css = lower.as_str();
    while let Some(at) = css.find("url(") {
        let target = css[at + 4..].trim_start().trim_start_matches(['"', '\'']);
        if !target.starts_with('#') {
            return refuse("external url()");
        }
        css = &css[at + 4..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn checked(appearance: serde_json::Value) -> Result<()> {
        serde_json::from_value::<OrganizationAppearance>(appearance)
            .expect("parses")
            .validate()
    }

    /// T-7031: page texts name a page and a language once, within the size cap.
    #[test]
    fn page_texts_name_a_page_and_language_once_within_the_cap() {
        let page = |page: &str, lang: &str, markdown: &str| json!({ "page": page, "lang": lang, "markdown": markdown });
        let fine = json!({ "pages": [
            page("connector", "sk", "# Konektor\n\n<script>x</script> shows as text"),
            page("connector", "en", "# Connector"),
        ]});
        assert!(checked(fine.clone()).is_ok(), "{:?}", checked(fine));
        let long = "a".repeat(crate::opendata_pages::MAX_PAGE_MARKDOWN + 1);
        for (pages, field) in [
            (
                json!([page("admin", "sk", "x")]),
                "spec.appearance.pages[0].page",
            ),
            (
                json!([page("terms", "SK", "x")]),
                "spec.appearance.pages[0].lang",
            ),
            (
                json!([page("terms", "svk", "x")]),
                "spec.appearance.pages[0].lang",
            ),
            (
                json!([page("terms", "sk", " \n")]),
                "spec.appearance.pages[0].markdown",
            ),
            (
                json!([page("terms", "sk", &long)]),
                "spec.appearance.pages[0].markdown",
            ),
            (
                json!([page("terms", "sk", "a\u{0}b")]),
                "spec.appearance.pages[0].markdown",
            ),
            (
                json!([page("terms", "sk", "a"), page("terms", "sk", "b")]),
                "spec.appearance.pages[1]",
            ),
        ] {
            match checked(json!({ "pages": pages })) {
                Err(Error::Invalid { field: got, .. }) => assert_eq!(got, field),
                other => panic!("{field}: {other:?}"),
            }
        }
        assert!(serde_json::from_value::<OrganizationAppearance>(
            json!({ "pages": [{ "page": "terms", "lang": "sk", "markdown": "x", "html": "<b>" }] })
        )
        .is_err());
    }

    /// T-7410: the theme and layout members take typed values only; a low-contrast pair, a
    /// `javascript:` or protocol-relative link and an oversized radius are refused by name.
    #[test]
    fn theme_and_layout_members_refuse_what_no_page_may_draw() {
        let fine = json!({
            "font": "serif",
            "radius": 12,
            "colours": { "background": "#0f172a", "text": "#f8fafc", "primary": "#93c5fd" },
            "hero": { "title": "Otvorené dáta", "intro": "Dáta mesta na **jednom mieste**. <script>x</script>" },
            "datasetList": "table",
            "navLinks": [{ "label": "Mesto", "url": "https://www.banskabystrica.sk" }],
            "footerLinks": [{ "label": "O nás", "url": "/about#api" }],
        });
        assert!(checked(fine.clone()).is_ok(), "{:?}", checked(fine));
        let link = |url: &str| json!({ "navLinks": [{ "label": "x", "url": url }] });
        for (appearance, field) in [
            (
                json!({ "colours": { "background": "#ffffff", "text": "#999999" } }),
                "spec.appearance.colours.text",
            ),
            (
                json!({ "colours": { "background": "#fff", "primary": "#ffff00" } }),
                "spec.appearance.colours.primary",
            ),
            (json!({ "radius": 25 }), "spec.appearance.radius"),
            (json!({ "hero": { "title": "<b>" } }), "spec.appearance.hero.title"),
            (json!({ "hero": { "intro": " " } }), "spec.appearance.hero.intro"),
            (link("javascript:alert(1)"), "spec.appearance.navLinks[0].url"),
            (link("JavaScript:alert(1)"), "spec.appearance.navLinks[0].url"),
            (link("data:text/html,x"), "spec.appearance.navLinks[0].url"),
            (link("//evil.example"), "spec.appearance.navLinks[0].url"),
            (link("/\\evil.example"), "spec.appearance.navLinks[0].url"),
            (link("https://"), "spec.appearance.navLinks[0].url"),
            (link("https://u@evil.example"), "spec.appearance.navLinks[0].url"),
            (link("/a\" onclick=\"x"), "spec.appearance.navLinks[0].url"),
            (link("about"), "spec.appearance.navLinks[0].url"),
            (
                json!({ "footerLinks": [{ "label": "", "url": "/" }] }),
                "spec.appearance.footerLinks[0].label",
            ),
            (
                json!({ "footerLinks": vec![json!({ "label": "x", "url": "/" }); 9] }),
                "spec.appearance.footerLinks[8]",
            ),
        ] {
            match checked(appearance.clone()) {
                Err(Error::Invalid { field: got, .. }) => assert_eq!(got, field, "{appearance}"),
                other => panic!("{field}: {other:?}"),
            }
        }
        for unknown in [
            json!({ "font": "Comic Sans" }),
            json!({ "datasetList": "grid" }),
            json!({ "hero": { "title": "x", "css": "x" } }),
            json!({ "navLinks": [{ "label": "x", "url": "/", "target": "_blank" }] }),
            json!({ "css": "body{}" }),
        ] {
            assert!(
                serde_json::from_value::<OrganizationAppearance>(unknown.clone()).is_err(),
                "{unknown}"
            );
        }
        let ratio = contrast_ratio("#000", "#ffffff").expect("hex");
        assert!((ratio - 21.0).abs() < 1e-9, "{ratio}");
    }

    /// T-7410: a Portal's appearance goes over its Organization's member by member.
    #[test]
    fn a_portal_appearance_goes_over_the_organizations_member_by_member() {
        let org: OrganizationAppearance = serde_json::from_value(json!({
            "instanceName": "Mesto",
            "colours": { "primary": "#0b5394", "text": "#111111" },
            "font": "serif",
            "hero": { "title": "Mesto", "intro": "Vitajte" },
            "navLinks": [{ "label": "Mesto", "url": "/" }],
        }))
        .expect("parses");
        let portal: OrganizationAppearance = serde_json::from_value(json!({
            "colours": { "primary": "#7a1f5c" },
            "hero": { "title": "Doprava" },
            "datasetList": "table",
        }))
        .expect("parses");
        let shown = portal.over(&org);
        assert_eq!(shown.instance_name.as_deref(), Some("Mesto"));
        assert_eq!(shown.colours.primary.as_deref(), Some("#7a1f5c"));
        assert_eq!(shown.colours.text.as_deref(), Some("#111111"));
        assert_eq!(shown.font, Some(AppearanceFont::Serif));
        assert_eq!(shown.hero.title.as_deref(), Some("Doprava"));
        assert_eq!(shown.hero.intro.as_deref(), Some("Vitajte"));
        assert_eq!(shown.dataset_list, Some(DatasetListLayout::Table));
        assert_eq!(shown.nav_links, org.nav_links);
        assert_eq!(OrganizationAppearance::default().over(&org), org);
    }

    /// T-6772 (UIX-16): an appearance is validated values, each refusal naming its member.
    #[test]
    fn an_appearance_takes_validated_values_and_names_the_refused_member() {
        let fine = json!({
            "instanceName": "Mesto Banská Bystrica",
            "colours": { "primary": "#0b5394", "text": "#000" },
            "licenseDefault": "CC-BY-4.0",
            "contactEmail": "opendata@banskabystrica.sk",
            "homeWidgets": ["doprava/prehlad", "ovzdusie/mapa"],
            "opendataWidgets": ["doprava/prehlad"],
            "logoSvg": "<svg xmlns=\"http://www.w3.org/2000/svg\"><use href=\"#a\"/><rect fill=\"url(#g)\" width=\"1\"/></svg>",
        });
        assert!(checked(fine.clone()).is_ok(), "{:?}", checked(fine));

        for (appearance, field) in [
            (
                json!({ "colours": { "primary": "red;}body{background:url(//x)}" } }),
                "spec.appearance.colours.primary",
            ),
            (
                json!({ "colours": { "text": "#12345" } }),
                "spec.appearance.colours.text",
            ),
            (
                json!({ "instanceName": "<b>x</b>" }),
                "spec.appearance.instanceName",
            ),
            (json!({ "shortName": " " }), "spec.appearance.shortName"),
            (json!({ "city": "a\nb" }), "spec.appearance.city"),
            (
                json!({ "contactEmail": "not-an-address" }),
                "spec.appearance.contactEmail",
            ),
            (
                json!({ "contactEmail": "a@b@c.sk" }),
                "spec.appearance.contactEmail",
            ),
            (
                json!({ "licenseDefault": "CC BY" }),
                "spec.appearance.licenseDefault",
            ),
            (
                json!({ "logoSvg": "<svg><script>alert(1)</script></svg>" }),
                "spec.appearance.logoSvg",
            ),
            (
                json!({ "logoSvg": "<svg onload=\"x()\"></svg>" }),
                "spec.appearance.logoSvg",
            ),
            (
                json!({ "logoSvg": "<svg><image href=\"https://evil.example/a.png\"/></svg>" }),
                "spec.appearance.logoSvg",
            ),
            (
                json!({ "logoSvg": "<svg><image xlink:href = 'data:image/png;base64,AA'/></svg>" }),
                "spec.appearance.logoSvg",
            ),
            (
                json!({ "faviconSvg": "<svg><rect style=\"fill:url(https://x)\"/></svg>" }),
                "spec.appearance.faviconSvg",
            ),
            (
                json!({ "faviconSvg": "<html></html>" }),
                "spec.appearance.faviconSvg",
            ),
            (
                json!({ "homeWidgets": ["doprava/prehlad", "Doprava/x"] }),
                "spec.appearance.homeWidgets[1]",
            ),
            (
                json!({ "homeWidgets": ["prehlad"] }),
                "spec.appearance.homeWidgets[0]",
            ),
            (
                json!({ "homeWidgets": ["a/b", "a/b"] }),
                "spec.appearance.homeWidgets[1]",
            ),
            (
                json!({ "homeWidgets": ["a/a", "a/b", "a/c", "a/d", "a/e", "a/f", "a/g"] }),
                "spec.appearance.homeWidgets[6]",
            ),
            (
                json!({ "opendataWidgets": ["doprava/prehlad", "doprava/prehlad"] }),
                "spec.appearance.opendataWidgets[1]",
            ),
            (
                json!({ "opendataWidgets": ["prehlad"] }),
                "spec.appearance.opendataWidgets[0]",
            ),
            (
                json!({ "faviconSvg": format!("<svg>{}</svg>", "a".repeat(MAX_APPEARANCE_SVG)) }),
                "spec.appearance.faviconSvg",
            ),
        ] {
            let refusal = checked(appearance.clone()).expect_err(field).to_string();
            assert!(refusal.contains(field), "{appearance}: {refusal}");
        }
        assert!(
            serde_json::from_value::<OrganizationAppearance>(json!({ "css": "body{}" })).is_err(),
            "raw CSS has no member"
        );
    }

    /// Words that merely contain `on` or `href` are no handler and no reference.
    #[test]
    fn an_svg_naming_on_or_href_in_text_is_not_refused() {
        let svg = "<svg><title>Montonero, the long one</title><g id=\"done\"><text>hrefs</text></g></svg>";
        assert!(checked(json!({ "logoSvg": svg })).is_ok());
    }
}
