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
        for (at, widget) in self.home_widgets.iter().enumerate() {
            let field = format!("spec.appearance.homeWidgets[{at}]");
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
            if self.home_widgets[..at].contains(widget) {
                return Err(invalid(&field, "a Dashboard not named twice"));
            }
        }
        Ok(())
    }
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

    /// T-6772 (UIX-16): an appearance is validated values, each refusal naming its member.
    #[test]
    fn an_appearance_takes_validated_values_and_names_the_refused_member() {
        let fine = json!({
            "instanceName": "Mesto Banská Bystrica",
            "colours": { "primary": "#0b5394", "text": "#000" },
            "licenseDefault": "CC-BY-4.0",
            "contactEmail": "opendata@banskabystrica.sk",
            "homeWidgets": ["doprava/prehlad", "ovzdusie/mapa"],
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
