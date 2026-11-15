#![forbid(unsafe_code)]
//! Built-in view renderers, the view-HTML sanitizer and the view payload check (`C10`).
//!
//! No plugin may decide authorization (`IAuthFunctions`, `IPermissionLabels`, `IAuthenticator`,
//! `IApiToken`, `IMiddleware` are intentionally not offered, D4).

use dataroom_core::{DataroomError, Dataset, Distribution, ResourceView, Result};
use serde_json::{json, Value};

/// Validates that a `resource_view_create` or `resource_view_update` payload contains no
/// undeclared `__extras` fields (matching `ours/platform/jcctl-publish/ckan.rs` `VIEW_PAYLOAD`, EP-144).
pub fn validate_view_payload(payload: &Value) -> Result<()> {
    const CORE_FIELDS: [&str; 6] = [
        "id",
        "resource_id",
        "view_type",
        "title",
        "description",
        "order",
    ];
    const JC_VIS_FIELDS: [&str; 4] = ["jc_name", "spec", "source", "endpoint"];

    let Some(obj) = payload.as_object() else {
        return Err(DataroomError::field_error(
            "view",
            "payload must be a JSON object",
        ));
    };
    let view_type = obj.get("view_type").and_then(Value::as_str).unwrap_or("");
    for key in obj.keys() {
        if CORE_FIELDS.contains(&key.as_str()) {
            continue;
        }
        if view_type == "jc_visualization" && JC_VIS_FIELDS.contains(&key.as_str()) {
            continue;
        }
        if matches!(
            view_type,
            "datatables_view" | "image_view" | "text_view" | "map_view"
        ) && matches!(key.as_str(), "image_url" | "page_url" | "filterable")
        {
            continue;
        }
        return Err(DataroomError::field_error(
            "__extras",
            format!("unexpected view field '{key}' for view_type '{view_type}'"),
        ));
    }
    Ok(())
}

/// Tags a view plugin may return; anything else escapes the whole fragment.
const VIEW_TAGS: &[&str] = &[
    "a",
    "abbr",
    "b",
    "br",
    "caption",
    "code",
    "dd",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "li",
    "ol",
    "p",
    "pre",
    "section",
    "small",
    "span",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
];

/// Attributes a view plugin may set besides `data-*` and `aria-*`. No `style`, no `on*`.
const VIEW_ATTRS: &[&str] = &[
    "alt", "class", "colspan", "dir", "height", "href", "id", "lang", "loading", "rowspan", "src",
    "title", "width",
];

/// Sanitizes an HTML fragment returned by a view plugin. The fragment passes unchanged only
/// when every tag is in [`VIEW_TAGS`], every attribute in [`VIEW_ATTRS`] (or `data-*`,
/// `aria-*`) and every `href`/`src` is an http(s) or relative URL; otherwise the whole fragment
/// is escaped and shows as text. So `<script>`, `<iframe>`, `<style>`, `<object>`, `<embed>`,
/// `<form>`, `<base>`, comments, `on*` handlers and `javascript:` URLs (entity-encoded too)
/// never reach the page as markup.
pub fn sanitize_view_html(raw: &str) -> String {
    if fragment_is_safe(raw) {
        raw.to_owned()
    } else {
        html_escape(raw)
    }
}

fn fragment_is_safe(html: &str) -> bool {
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let Some(end) = tag_end(rest) else {
            return false;
        };
        if !tag_is_safe(&rest[..end]) {
            return false;
        }
        rest = &rest[end + 1..];
    }
    // A bare `>` is harmless text; an unclosed `<` was refused above.
    true
}

/// Index of the `>` closing a tag, skipping quoted attribute values.
fn tag_end(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in tag.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn tag_is_safe(tag: &str) -> bool {
    let body = tag.strip_prefix('/').unwrap_or(tag);
    let name_len = body
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(body.len());
    let name = body[..name_len].to_ascii_lowercase();
    if !VIEW_TAGS.contains(&name.as_str()) {
        return false;
    }
    let mut attrs = &body[name_len..];
    loop {
        attrs = attrs.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        if attrs.is_empty() {
            return true;
        }
        let key_len = attrs
            .find(|c: char| c.is_ascii_whitespace() || matches!(c, '=' | '/' | '>'))
            .unwrap_or(attrs.len());
        let key = attrs[..key_len].to_ascii_lowercase();
        attrs = attrs[key_len..].trim_start();
        let mut value = "";
        if let Some(after) = attrs.strip_prefix('=') {
            let after = after.trim_start();
            let (v, tail) = match after.chars().next() {
                Some(q @ ('"' | '\'')) => match after[1..].find(q) {
                    Some(i) => (&after[1..=i], &after[i + 2..]),
                    None => return false,
                },
                _ => {
                    let i = after
                        .find(|c: char| c.is_ascii_whitespace())
                        .unwrap_or(after.len());
                    (&after[..i], &after[i..])
                }
            };
            value = v;
            attrs = tail;
        }
        let known = VIEW_ATTRS.contains(&key.as_str())
            || key.starts_with("data-")
            || key.starts_with("aria-");
        if key.is_empty() || !known {
            return false;
        }
        if matches!(key.as_str(), "href" | "src") && !attr_url_is_safe(value) {
            return false;
        }
    }
}

/// An attribute URL is safe when it holds no character reference but `&amp;` (a browser decodes
/// `&#106;avascript:` before it reads the scheme) and its scheme is http(s) or absent.
fn attr_url_is_safe(value: &str) -> bool {
    let mut rest = value;
    while let Some(i) = rest.find('&') {
        if !rest[i..].starts_with("&amp;") {
            return false;
        }
        rest = &rest[i + 1..];
    }
    url_is_safe(&value.replace("&amp;", "&"))
}

/// A link target a page may carry: http(s), or relative with no scheme. Browsers drop ASCII
/// tabs, newlines and leading controls before reading a scheme, so they are dropped here too.
fn url_is_safe(url: &str) -> bool {
    let plain: String = url
        .chars()
        .filter(|c| !c.is_ascii_control() && *c != ' ')
        .collect::<String>()
        .to_ascii_lowercase();
    let scheme_end = plain.find([':', '/', '?', '#']);
    match scheme_end {
        Some(i) if plain[i..].starts_with(':') => {
            matches!(&plain[..i], "http" | "https")
        }
        _ => true,
    }
}

/// `url` when a page may link to it, else an empty attribute value.
fn safe_link(url: &str) -> String {
    if url_is_safe(url) {
        html_escape(url)
    } else {
        String::new()
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Renders a resource view into a safe HTML fragment (including SDK grid config for `datatables_view`).
pub fn render_resource_view(view: &ResourceView, dist: &Distribution, dataset: &Dataset) -> String {
    match view.view_type.as_str() {
        "datatables_view" => {
            let grid_cfg = json!({
                "source": {
                    "kind": "odata",
                    "url": "/odata/v4",
                    "entitySet": "Distributions"
                },
                "type": "Distribution",
                "pageSize": 50,
                "mode": "view",
                "density": "comfortable"
            });
            format!(
                "<section class=\"dr-view dr-view-table\" data-sdk-grid=\"{}\"><h4>{}</h4><p><a href=\"{}\">{}</a></p></section>",
                html_escape(&grid_cfg.to_string()),
                html_escape(&view.title),
                safe_link(&dist.access_url),
                html_escape(&dataset.name)
            )
        }
        "jc_visualization" => {
            let endpoint = view
                .config
                .get("endpoint")
                .and_then(Value::as_str)
                .unwrap_or("");
            format!(
                "<section class=\"dr-view dr-view-chart\" data-endpoint=\"{}\"><h4>{}</h4><p>{}</p></section>",
                html_escape(endpoint),
                html_escape(&view.title),
                html_escape(&view.description)
            )
        }
        "image_view" => format!(
            "<figure class=\"dr-view dr-view-image\"><img src=\"{}\" alt=\"{}\" loading=\"lazy\"/></figure>",
            safe_link(&dist.access_url),
            html_escape(&view.title)
        ),
        _ => format!(
            "<section class=\"dr-view\"><h4>{}</h4><p>{}</p></section>",
            html_escape(&view.title),
            html_escape(&view.description)
        ),
    }
}
