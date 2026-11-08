#![forbid(unsafe_code)]
//! Plugin traits, built-in view renderers, and WASM host limits (`C10`, `ours/platform/wasm-host-limits.rs`).
//!
//! No plugin may decide authorization (`IAuthFunctions`, `IPermissionLabels`, `IAuthenticator`,
//! `IApiToken`, `IMiddleware` are intentionally not offered, D4).

use dataroom_core::{DataroomError, Dataset, Distribution, ResourceView, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

/// Epoch tick duration from `ours/platform/wasm-host-limits.rs`.
pub const EPOCH_TICK: Duration = Duration::from_millis(10);

/// Platform shard WASM limits from `ours/platform/wasm-host-limits.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformWasmLimits {
    pub memory_bytes: usize,
    pub wall_time: Duration,
    pub fuel: u64,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub per_app_concurrency: usize,
    pub per_tenant_concurrency: usize,
    pub cached_components: usize,
    pub pooled_instances: u32,
}

impl Default for PlatformWasmLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 64 << 20,
            wall_time: Duration::from_secs(5),
            fuel: 200_000_000_000,
            request_bytes: 2 << 20,
            response_bytes: 8 << 20,
            per_app_concurrency: 16,
            per_tenant_concurrency: 64,
            cached_components: 2_000,
            pooled_instances: 1_000,
        }
    }
}

/// Catalogue plugin execution limits (C10, research-ckan D7):
/// 16 MiB linear memory, 1 G fuel per call, 1 s wall time per hook, 30 s per harvest import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CataloguePluginLimits {
    pub memory_bytes: usize,
    pub fuel_per_call: u64,
    pub hook_wall_time: Duration,
    pub harvest_import_wall_time: Duration,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub per_plugin_concurrency: usize,
    pub per_tenant_concurrency: usize,
}

impl Default for CataloguePluginLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 16 << 20,
            fuel_per_call: 1_000_000_000,
            hook_wall_time: Duration::from_secs(1),
            harvest_import_wall_time: Duration::from_secs(30),
            request_bytes: 2 << 20,
            response_bytes: 8 << 20,
            per_plugin_concurrency: 16,
            per_tenant_concurrency: 64,
        }
    }
}

impl CataloguePluginLimits {
    pub fn hook_deadline_ticks(&self) -> u64 {
        (self.hook_wall_time.as_millis() / EPOCH_TICK.as_millis()).max(1) as u64
    }

    pub fn harvest_deadline_ticks(&self) -> u64 {
        (self.harvest_import_wall_time.as_millis() / EPOCH_TICK.as_millis()).max(1) as u64
    }
}

/// WIT world descriptor for `jc:catalogue-plugin` components (research-ckan §4.4).
pub const CATALOGUE_PLUGIN_WIT: &str = r#"
package jc:catalogue-plugin@0.1.0;

interface types {
    record validation-finding {
        path: string,
        message: string,
    }
}

world catalogue-plugin {
    use types.{validation-finding};
    export validate: func(dataset-json: string) -> list<validation-finding>;
    export index-terms: func(dataset-json: string) -> list<string>;
    export show-transform: func(dataset-json: string) -> string;
    export render-view: func(view-json: string, resource-json: string) -> string;
}
"#;

/// Dataset lifecycle hook trait implemented by built-in and WASM plugins.
///
/// Notice that plugins cannot decide authorization (D4).
pub trait DatasetHookPlugin: Send + Sync {
    fn name(&self) -> &str;

    /// Validates a dataset before write; returns `(field_path, message)` findings.
    fn validate(&self, _dataset: &Dataset) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Returns extra full-text search terms for indexing.
    fn index_terms(&self, _dataset: &Dataset) -> Vec<String> {
        Vec::new()
    }

    /// Pure transformation limited to plugin-owned keys (`x_<plugin>_*`).
    fn show_transform(&self, _dataset: &Dataset) -> serde_json::Map<String, Value> {
        serde_json::Map::new()
    }
}

/// Resource view plugin trait (`IResourceView` equivalent, C10).
pub trait ViewPlugin: Send + Sync {
    fn view_type(&self) -> &str;
    fn title(&self) -> &str;
    fn allowed_fields(&self) -> &[&str];
    fn render_html(&self, view: &ResourceView, dist: &Distribution, dataset: &Dataset) -> String;
}

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
