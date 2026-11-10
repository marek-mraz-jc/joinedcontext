#![forbid(unsafe_code)]
//! The page every screen a person sees in kaisatsu is built on (UI-82, PF-90): keycloak.v2's
//! login layout with the joinedcontext theme old prod ran on Keycloak 26.8.1 — the logo and the
//! organization above the card, "Sign in to {app}" as a sign-in page's heading and tab title,
//! the app's name above every other page's heading, the language picker, PatternFly's form
//! fields. The look is one stylesheet served from kaisatsu's own origin ([`LOGIN_CSS`]); the
//! branding block's colours and fonts are custom properties in the page, under its CSP nonce.

use crate::branding::BrandingBlock;
use crate::i18n::Bundle;
use crate::model::RealmConfig;
use askama::Template;

/// PatternFly's subset, keycloak.v2's styles.css and the theme's joinedcontext.css (header of
/// the file says how the subset was taken).
pub const LOGIN_CSS: &str = include_str!("../static/login.css");

const fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
        i += 1;
    }
    hash
}

/// The stylesheet's version in its URL, so a page never meets an older cached copy.
pub const LOGIN_CSS_VERSION: u64 = fnv1a(LOGIN_CSS.as_bytes());

/// Where the stylesheet is served under a mount's path prefix.
#[must_use]
pub fn css_url(path_prefix: &str) -> String {
    format!("{path_prefix}/resources/kaisatsu/{LOGIN_CSS_VERSION:016x}/login.css")
}

/// Where a realm's logo is served (only when its branding block holds one).
#[must_use]
pub fn logo_url(path_prefix: &str, realm: &str) -> String {
    format!("{path_prefix}/realms/{realm}/login-resources/logo.svg")
}

// Font Awesome Free 5 paths (CC BY 4.0), as PatternFly's React icons draw them.
pub const ICON_ERROR: &str = r#"<svg class="pf-v5-svg" viewBox="0 0 512 512" fill="currentColor" aria-hidden="true" width="1em" height="1em"><path d="M504 256c0 136.997-111.043 248-248 248S8 392.997 8 256C8 119.083 119.043 8 256 8s248 111.083 248 248zm-248 50c-25.405 0-46 20.595-46 46s20.595 46 46 46 46-20.595 46-46-20.595-46-46-46zm-43.673-165.346l7.418 136c.347 6.364 5.609 11.346 11.982 11.346h48.546c6.373 0 11.635-4.982 11.982-11.346l7.418-136c.375-6.874-5.098-12.654-11.982-12.654h-63.383c-6.884 0-12.356 5.78-11.981 12.654z"></path></svg>"#;
pub const ICON_WARNING: &str = r#"<svg class="pf-v5-svg" viewBox="0 0 576 512" fill="currentColor" aria-hidden="true" width="1em" height="1em"><path d="M569.517 440.013C587.975 472.007 564.806 512 527.94 512H48.054c-36.937 0-59.999-40.055-41.577-71.987L246.423 23.985c18.467-32.009 64.72-31.951 83.154 0l239.94 416.028zM288 354c-25.405 0-46 20.595-46 46s20.595 46 46 46 46-20.595 46-46-20.595-46-46-46zm-43.673-165.346l7.418 136c.347 6.364 5.609 11.346 11.982 11.346h48.546c6.373 0 11.635-4.982 11.982-11.346l7.418-136c.375-6.874-5.098-12.654-11.982-12.654h-63.383c-6.884 0-12.356 5.78-11.981 12.654z"></path></svg>"#;
pub const ICON_SUCCESS: &str = r#"<svg class="pf-v5-svg" viewBox="0 0 512 512" fill="currentColor" aria-hidden="true" width="1em" height="1em"><path d="M504 256c0 136.967-111.033 248-248 248S8 392.967 8 256 119.033 8 256 8s248 111.033 248 248zM227.314 387.314l184-184c6.248-6.248 6.248-16.379 0-22.627l-22.627-22.627c-6.248-6.249-16.379-6.249-22.628 0L216 308.118l-70.059-70.059c-6.248-6.248-16.379-6.248-22.628 0l-22.627 22.627c-6.248 6.248-6.248 16.379 0 22.627l104 104c6.249 6.249 16.379 6.249 22.628.001z"></path></svg>"#;
pub const ICON_INFO: &str = r#"<svg class="pf-v5-svg" viewBox="0 0 512 512" fill="currentColor" aria-hidden="true" width="1em" height="1em"><path d="M256 8C119.043 8 8 119.083 8 256c0 136.997 111.043 248 248 248s248-111.003 248-248C504 119.083 392.957 8 256 8zm0 110c23.196 0 42 18.804 42 42s-18.804 42-42 42-42-18.804-42-42 18.804-42 42-42zm56 254c0 6.627-5.373 12-12 12h-88c-6.627 0-12-5.373-12-12v-24c0-6.627 5.373-12 12-12h12v-64h-12c-6.627 0-12-5.373-12-12v-24c0-6.627 5.373-12 12-12h64c6.627 0 12 5.373 12 12v100h12c6.627 0 12 5.373 12 12v24z"></path></svg>"#;
pub const ICON_EYE: &str = r#"<svg class="pf-v5-svg" viewBox="0 0 576 512" fill="currentColor" aria-hidden="true" width="1em" height="1em"><path d="M572.52 241.4C518.29 135.59 410.93 64 288 64S57.68 135.64 3.48 241.41a32.35 32.35 0 0 0 0 29.19C57.71 376.41 165.07 448 288 448s230.32-71.64 284.52-177.41a32.35 32.35 0 0 0 0-29.19zM288 400a144 144 0 1 1 144-144 143.930 143.930 0 0 1-144 144zm0-240a95.31 95.31 0 0 0-25.31 3.79 47.85 47.85 0 0 1-66.9 66.9A95.78 95.78 0 1 0 288 160z"></path></svg>"#;
pub const ICON_EYE_SLASH: &str = r#"<svg class="pf-v5-svg" viewBox="0 0 640 512" fill="currentColor" aria-hidden="true" width="1em" height="1em"><path d="M320 400c-75.85 0-137.25-58.71-142.9-133.11L72.2 185.82c-13.79 17.3-26.48 35.59-36.72 55.59a32.35 32.35 0 0 0 0 29.19C89.71 376.41 197.07 448 320 448c26.910 0 52.87-4 77.89-10.46L346 397.39a144.13 144.13 0 0 1-26 2.61zm313.82 58.1l-110.55-85.44a331.25 331.25 0 0 0 81.25-102.07 32.35 32.35 0 0 0 0-29.19C550.29 135.59 442.93 64 320 64a308.15 308.15 0 0 0-147.32 37.7L45.46 3.37A16 16 0 0 0 23 6.18L3.37 31.45A16 16 0 0 0 6.18 53.9l588.36 454.73a16 16 0 0 0 22.46-2.81l19.640-25.27a16 16 0 0 0-2.82-22.45zm-183.72-142l-39.3-30.38A94.75 94.75 0 0 0 416 256a94.76 94.76 0 0 0-121.31-92.21A47.65 47.65 0 0 1 304 192a46.64 46.64 0 0 1-1.54 10l-73.61-56.89A142.31 142.31 0 0 1 320 112a143.92 143.92 0 0 1 144 144c0 21.63-5.29 41.79-13.9 60.11z"></path></svg>"#;
pub const ICON_SYNC: &str = r#"<svg class="pf-v5-svg" viewBox="0 0 512 512" fill="currentColor" aria-hidden="true" width="1em" height="1em"><path d="M370.72 133.28C339.458 104.008 298.888 87.962 255.848 88c-77.458.068-144.328 53.178-162.791 126.85-1.344 5.363-6.122 9.15-11.651 9.15H24.103c-7.498 0-13.194-6.807-11.807-14.176C33.933 94.924 134.813 8 256 8c66.448 0 126.791 26.136 171.315 68.685L463.03 40.97C478.149 25.851 504 36.559 504 57.941V192c0 13.255-10.745 24-24 24H345.941c-21.382 0-32.09-25.851-16.971-40.971l41.75-41.749zM32 296h134.059c21.382 0 32.09 25.851 16.971 40.971l-41.75 41.75c31.262 29.273 71.835 45.319 114.876 45.28 77.418-.07 144.315-53.144 162.787-126.849 1.344-5.363 6.122-9.15 11.651-9.15h57.304c7.498 0 13.194 6.807 11.807 14.176C478.067 417.076 377.187 504 256 504c-66.448 0-126.791-26.136-171.315-68.685L48.97 471.03C33.851 486.149 8 475.441 8 454.059V320c0-13.255 10.745-24 24-24z"></path></svg>"#;

/// The banner above a page's form: Keycloak's `message` with its type.
#[derive(Debug, Clone)]
pub struct Alert {
    /// PatternFly's modifier: `danger`, `warning`, `success` or `info`.
    pub kind: &'static str,
    pub text: String,
}

impl Alert {
    #[must_use]
    pub fn danger(text: impl Into<String>) -> Self {
        Self {
            kind: "danger",
            text: text.into(),
        }
    }
    #[must_use]
    pub fn warning(text: impl Into<String>) -> Self {
        Self {
            kind: "warning",
            text: text.into(),
        }
    }
    #[must_use]
    pub fn success(text: impl Into<String>) -> Self {
        Self {
            kind: "success",
            text: text.into(),
        }
    }
    #[must_use]
    pub fn icon(&self) -> &'static str {
        match self.kind {
            "danger" => ICON_ERROR,
            "warning" => ICON_WARNING,
            "success" => ICON_SUCCESS,
            _ => ICON_INFO,
        }
    }
}

/// The person being signed in, shown read-only above a second step with a way to start over.
#[derive(Debug, Clone)]
pub struct Attempted {
    pub label: String,
    pub username: String,
    pub restart_url: String,
    pub restart_label: String,
}

/// The authenticator set-up steps of `login-config-totp`.
#[derive(Debug, Clone, Default)]
pub struct TotpSetup {
    pub step1: String,
    pub apps: Vec<String>,
    pub step2: String,
    /// Our own inline SVG QR code (rects only, never input).
    pub qr_svg: String,
    pub unable_to_scan: String,
    pub manual_step2: String,
    pub key: String,
    pub manual_step3: String,
    pub config: Vec<String>,
    pub step3: String,
    pub step3_device: String,
}

#[derive(Debug, Clone, Default)]
pub struct Field {
    pub name: String,
    pub label: String,
    pub input_type: String,
    pub value: String,
    pub autocomplete: String,
    pub error: String,
    pub required: bool,
    pub autofocus: bool,
    /// A link under the field (the sign-in page's "Forgot your password?").
    pub link_id: String,
    pub link_url: String,
    pub link_label: String,
}

#[derive(Debug, Clone, Default)]
pub struct Form {
    pub id: String,
    pub action: String,
    pub hidden: Vec<(String, String)>,
    pub fields: Vec<Field>,
    pub submit_name: String,
    /// The submit button's value, when the form tells two buttons apart (Yes/No).
    pub submit_value: String,
    pub submit_id: String,
    pub submit_label: String,
    /// With `secondary_value`, the secondary action is a second submit button of the same name.
    pub secondary_value: String,
    pub secondary_id: String,
    pub secondary_url: String,
    pub secondary_label: String,
}

/// A button-styled link in the card (`pf-m-primary`, `pf-m-link`, `pf-m-secondary`).
#[derive(Debug, Clone)]
pub struct Link {
    pub id: String,
    pub class: &'static str,
    pub url: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct LocaleChoice {
    pub label: String,
    pub url: String,
    pub selected: bool,
}

#[derive(Template, Debug, Clone, Default)]
#[template(path = "login_page.html")]
pub struct Page {
    pub lang: String,
    pub title: String,
    pub org: String,
    pub logo_url: String,
    pub app_context: String,
    pub heading: String,
    pub page_id: String,
    pub nonce: String,
    pub css_url: String,
    pub branding_css: String,
    pub locales: Vec<LocaleChoice>,
    pub languages_label: String,
    pub alert: Option<Alert>,
    pub attempted: Option<Attempted>,
    pub totp: Option<TotpSetup>,
    pub instructions: Vec<String>,
    pub form: Option<Form>,
    pub links: Vec<Link>,
    pub info: Vec<String>,
    pub show_label: String,
    pub hide_label: String,
}

impl Page {
    /// A heading known only after the page was made (it names the app): the tab title follows.
    pub fn set_heading(&mut self, heading: String) {
        self.title = format!("{heading} · {}", self.org);
        self.heading = heading;
    }

    /// The rendered page; empty only when a template value fails to render.
    #[must_use]
    pub fn html(&self) -> String {
        self.render().unwrap_or_default()
    }
}

/// The pages whose tab title is "Sign in to {app} · {org}" and whose heading names the app
/// (old theme's jc.ftl `signInPage`); every other page shows the app above its heading.
pub const SIGN_IN_PAGES: [&str; 1] = ["login"];

/// What the shell of every page needs: the realm's names, its branding and the language.
pub struct Shell<'a> {
    pub bundle: &'a Bundle,
    pub realm: &'a RealmConfig,
    pub branding: &'a BrandingBlock,
    pub path_prefix: &'a str,
    /// The app's name (client name translated, `the Portal`), empty when no client is known.
    pub app: &'a str,
    /// The page's own URL without `kc_locale`, for the language picker; empty: no picker.
    pub locale_base_url: &'a str,
}

impl Shell<'_> {
    /// The organization above the card: the realm's display name, else the branding block's
    /// organisation or instance name (jc's own realm carries none), else "this platform".
    #[must_use]
    pub fn org(&self) -> String {
        [
            self.realm.display_name.as_str(),
            self.branding.organisation.as_str(),
            self.branding.instance_name.as_str(),
        ]
        .into_iter()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map_or_else(
            || self.bundle.get("jcOrganizationFallback").to_owned(),
            str::to_owned,
        )
    }

    /// A page with the shell filled: a sign-in page (`page_id` in [`SIGN_IN_PAGES`]) with an app
    /// is titled "Sign in to {app} · {org}" and headed "Sign in to {app}"; any other page keeps
    /// `heading`, is titled "{heading} · {org}" and names the app above the heading.
    #[must_use]
    pub fn page(&self, page_id: &str, heading: &str, nonce: &str) -> Page {
        let org = self.org();
        let sign_in = !self.app.is_empty() && SIGN_IN_PAGES.contains(&page_id);
        let (title, heading, app_context) = if sign_in {
            (
                self.bundle.format("jcSignInTo", &[self.app, &org]),
                self.bundle.format("jcSignInToApp", &[self.app]),
                String::new(),
            )
        } else {
            (
                format!("{heading} · {org}"),
                heading.to_owned(),
                cap_first(self.app),
            )
        };
        let logo_url = if self.branding.logo_svg.is_empty() {
            String::new()
        } else {
            logo_url(self.path_prefix, &self.realm.realm)
        };
        Page {
            lang: self.bundle.locale.to_owned(),
            title,
            org,
            logo_url,
            app_context,
            heading,
            page_id: page_id.to_owned(),
            nonce: nonce.to_owned(),
            css_url: css_url(self.path_prefix),
            branding_css: self.branding.css_variables(),
            locales: self.locales(),
            languages_label: self.bundle.get("languages").to_owned(),
            show_label: self.bundle.get("showPassword").to_owned(),
            hide_label: self.bundle.get("hidePassword").to_owned(),
            ..Page::default()
        }
    }

    /// The realm's languages that have a bundle, labelled in the page's language and sorted by
    /// label, as keycloak.v2's picker lists them; one language or none shows no picker.
    fn locales(&self) -> Vec<LocaleChoice> {
        if !self.realm.internationalization_enabled || self.locale_base_url.is_empty() {
            return Vec::new();
        }
        let sep = if self.locale_base_url.contains('?') {
            '&'
        } else {
            '?'
        };
        let mut out: Vec<LocaleChoice> = crate::i18n::realm_locales(self.realm)
            .into_iter()
            .map(|l| LocaleChoice {
                label: self.bundle.get(&format!("locale_{l}")).to_owned(),
                url: format!("{}{sep}kc_locale={l}", self.locale_base_url),
                selected: l == self.bundle.locale,
            })
            .collect();
        out.sort_by(|a, b| a.label.cmp(&b.label));
        out
    }
}

/// `the Portal` -> `The Portal` (FreeMarker's `cap_first`).
#[must_use]
pub fn cap_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().chain(chars).collect::<String>()
    })
}

/// The headers of every page: a CSP with the page's nonce and no other script or style
/// source than the page's own origin, no framing, no caching.
#[must_use]
pub fn security_headers(nonce: &str, form_action: &str) -> [(http::HeaderName, String); 5] {
    use http::header;
    [
        (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
        (
            header::CONTENT_SECURITY_POLICY,
            format!(
                "default-src 'self'; script-src 'nonce-{nonce}'; style-src 'self' 'nonce-{nonce}'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action {form_action}"
            ),
        ),
        (header::X_FRAME_OPTIONS, "DENY".to_owned()),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        (
            header::CACHE_CONTROL,
            "no-store, no-cache, must-revalidate".to_owned(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stylesheet_names_no_other_origin_and_no_url() {
        assert!(
            !LOGIN_CSS.contains("url("),
            "the page loads nothing but the sheet"
        );
        assert!(!LOGIN_CSS.contains("@import"));
        assert!(
            LOGIN_CSS.contains("--pf-v5-c-button--m-primary--BackgroundColor: var(--jc-primary)")
        );
    }

    #[test]
    fn cap_first_capitalises_one_letter() {
        assert_eq!(cap_first("the Portal"), "The Portal");
        assert_eq!(cap_first(""), "");
        assert_eq!(cap_first("účet"), "Účet");
    }
}
