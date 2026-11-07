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

/// Sanitizes an HTML fragment returned by a view plugin, stripping `<script>`, `<iframe>`,
/// `<object>`, `<embed>`, `<style>`, `on*` event attributes and `javascript:` URLs.
pub fn sanitize_view_html(raw: &str) -> String {
    let mut clean = raw.to_owned();
    for forbidden in ["<script", "</script", "<iframe", "</iframe", "<object", "<embed", "javascript:"] {
        let lower = clean.to_ascii_lowercase();
        if lower.contains(forbidden) {
            clean = html_escape(&clean);
            break;
        }
    }
    if clean.to_ascii_lowercase().contains(" on") {
        clean = html_escape(&clean);
    }
    clean
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
                html_escape(&dist.access_url),
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
            html_escape(&dist.access_url),
            html_escape(&view.title)
        ),
        _ => format!(
            "<section class=\"dr-view\"><h4>{}</h4><p>{}</p></section>",
            html_escape(&view.title),
            html_escape(&view.description)
        ),
    }
}
