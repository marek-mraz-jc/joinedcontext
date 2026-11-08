//! The templates keep CKAN's markup (Bootstrap and CKAN layout classes); every such class they use
//! needs a rule in the served stylesheet, or the page falls apart into bullet lists and one column
//! (the jc-parity comparison of 2026-11-08 found the masthead, breadcrumb and footer unstyled).

use std::collections::BTreeSet;

/// Classes the templates use only as hooks for scripts, tests or the theme's descendant rules.
const HOOKS: &[&str] = &[
    "btn-lg",
    "btn-sm",
    "collapse",
    "control-order-by",
    "count",
    "filtered",
    "filters",
    "flash-messages",
    "home",
    "image",
    "input-lg",
    "lang-select",
    "logo",
    "media-image",
    "module-shallow",
    "navbar-light",
    "no-bottom-border",
    "no-nav",
    "not-authed",
    "nums",
    "pagination-wrapper",
    "prose",
    "resource-url-analytics",
    "resources",
    "sdk-datagrid",
    "shacl-panel",
    "show-filters",
];

fn template_classes() -> BTreeSet<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(dir).expect("templates") {
        let text = std::fs::read_to_string(entry.expect("entry").path()).expect("template");
        for chunk in text.split("class=\"").skip(1) {
            let value = chunk.split('"').next().unwrap_or_default();
            if value.contains('{') {
                continue; // a computed class list; its literal parts are covered where they are static
            }
            out.extend(value.split_whitespace().map(str::to_owned));
        }
    }
    out
}

fn styled(css: &str, class: &str) -> bool {
    let needle = format!(".{class}");
    css.match_indices(&needle).any(|(i, _)| {
        css[i + needle.len()..]
            .chars()
            .next()
            .is_some_and(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    })
}

#[test]
fn every_layout_class_the_templates_use_has_a_rule() {
    let css = dataroom_web::JC_THEME_CSS;
    let missing: Vec<_> = template_classes()
        .into_iter()
        .filter(|c| !c.starts_with("jc-") && !c.starts_with("fa") && !HOOKS.contains(&c.as_str()))
        .filter(|c| !styled(css, c))
        .collect();
    assert!(
        missing.is_empty(),
        "classes with no rule in JC_THEME_CSS: {missing:?}"
    );
}

#[test]
fn the_base_layout_comes_before_the_theme_so_the_theme_wins() {
    let css = dataroom_web::JC_THEME_CSS;
    let base = css.find(".row {").expect("base grid rule");
    let theme = css.find("--jc-paper").expect("theme tokens");
    assert!(base < theme);
    // The masthead's lists and the skip link stop rendering as bullets and plain text.
    for rule in [
        ".navbar-nav {",
        ".list-unstyled",
        ".breadcrumb {",
        ".visually-hidden-focusable:not(:focus)",
        // Buttons, fields and the pager keep Bootstrap's box; the brand never shrinks (the logo was cut).
        ".btn { display: inline-block",
        ".form-control, .form-select { display: block",
        ".page-link { display: block",
        ".jc-brand { flex-shrink: 0",
    ] {
        assert!(css.contains(rule), "{rule}");
    }
}

#[test]
fn the_default_logo_text_fits_its_box() {
    // At 18 px bold "joinedcontext" is wider than the 125 px left of the mark: the parity shots
    // showed "joinedconte" cut, so the word is held to its box.
    let svg = dataroom_web::JC_LOGO_SVG;
    let text = svg.split("<text x=\"46\"").nth(1).expect("the word");
    let len: f32 = text
        .split("textLength=\"")
        .nth(1)
        .and_then(|t| t.split('"').next())
        .and_then(|t| t.parse().ok())
        .expect("textLength on the word");
    assert!(46.0 + len <= 171.0, "{len}");
}
