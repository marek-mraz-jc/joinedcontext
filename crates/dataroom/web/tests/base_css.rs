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
    let mut dirs = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates")];
    let mut files = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).expect("templates") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                dirs.push(path); // templates/jc: the inlined brand marks
            } else {
                files.push(path);
            }
        }
    }
    let mut out = BTreeSet::new();
    for path in files {
        let text = std::fs::read_to_string(path).expect("template");
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
fn the_default_logo_draws_its_words_as_paths_so_nothing_is_cut() {
    // The parity shots showed the drawn stand-in's "joinedconte" cut; the lockup old prod served
    // (public__jc-logo.svg, 266x56) draws its words as paths, so no font can widen them.
    let svg = dataroom_web::JC_LOGO_SVG;
    assert!(svg.contains("viewBox=\"0 0 266 56\""), "the 266x56 lockup");
    assert!(
        svg.contains("<path") && !svg.contains("<text"),
        "words as paths"
    );
}
