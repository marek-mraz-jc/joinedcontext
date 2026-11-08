#![forbid(unsafe_code)]
//! Grafana classic dashboard JSON serde types at `schemaVersion: 42` (`kinds/dashboard/dashboard_kind.cue`).
//!
//! Every struct preserves unknown fields in a `BTreeMap<String, Value>` via `#[serde(flatten)]`
//! so that importing, migrating, and exporting any Grafana dashboard is lossless and byte-stable (R1, R2).

use crate::migration::LATEST_SCHEMA_VERSION;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Top-level Grafana classic dashboard document (v1, `schemaVersion: 42`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gnet_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_tooltip: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<TimeRangeSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timepicker: Option<TimePickerConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fiscal_year_start_month: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_now: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panels: Option<Vec<PanelSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub templating: Option<TemplatingContainer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AnnotationContainer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<DashboardLink>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preload: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

const fn default_schema_version() -> u16 {
    LATEST_SCHEMA_VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimeRangeSpec {
    pub from: String,
    pub to: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimePickerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_intervals: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_ranges: Option<Vec<Value>>,
    #[serde(rename = "nowDelay", default, skip_serializing_if = "Option::is_none")]
    pub now_delay: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataSourceRef {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub ds_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GridPos {
    #[serde(default = "default_grid_h")]
    pub h: u32,
    #[serde(default = "default_grid_w")]
    pub w: u32,
    #[serde(default)]
    pub x: u32,
    #[serde(default)]
    pub y: u32,
    #[serde(rename = "static", default, skip_serializing_if = "Option::is_none")]
    pub is_static: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

const fn default_grid_h() -> u32 {
    9
}
const fn default_grid_w() -> u32 {
    12
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPanelRef {
    pub name: String,
    pub uid: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MatcherConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DynamicConfigValue {
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataTransformerConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<MatcherConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default)]
    pub options: Value,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Threshold {
    pub value: Option<f64>,
    pub color: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThresholdsConfig {
    pub mode: String,
    #[serde(default)]
    pub steps: Vec<Threshold>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldColor {
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_by: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(
        rename = "displayNameFromDS",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub display_name_from_ds: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writeable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filterable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mappings: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<ThresholdsConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<FieldColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<BTreeMap<String, Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldOverride {
    #[serde(
        rename = "__systemRef",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub system_ref: Option<String>,
    pub matcher: MatcherConfig,
    #[serde(default)]
    pub properties: Vec<DynamicConfigValue>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldConfigSource {
    #[serde(default)]
    pub defaults: FieldConfig,
    #[serde(default)]
    pub overrides: Vec<FieldOverride>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Panel or RowPanel (`type: "row"`) in a classic dashboard (`#Panel | #RowPanel`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PanelSpec {
    #[serde(rename = "type")]
    pub panel_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<BTreeMap<String, Value>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transparent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource: Option<DataSourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_pos: Option<GridPos>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<DashboardLink>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_direction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_row: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_data_points: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transformations: Option<Vec<DataTransformerConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_shift: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide_time_override: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_compare: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_panel: Option<LibraryPanelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_timeout: Option<String>,
    #[serde(
        rename = "queryCachingTTL",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub query_caching_ttl: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_config: Option<FieldConfigSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed: Option<bool>,
    /// Nested panels when `type == "row"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panels: Option<Vec<PanelSpec>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TemplatingContainer {
    #[serde(default)]
    pub list: Vec<VariableModel>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VariableOption {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VariableModel {
    #[serde(rename = "type")]
    pub var_type: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_url_sync: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource: Option<DataSourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<VariableOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_custom_value: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<VariableOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_all: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationContainer {
    #[serde(default)]
    pub list: Vec<AnnotationQuery>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationQuery {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource: Option<DataSourceRef>,
    #[serde(default = "default_true")]
    pub enable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide: Option<bool>,
    #[serde(default)]
    pub icon_color: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardLink {
    pub title: String,
    #[serde(rename = "type")]
    pub link_type: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub tooltip: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub as_dropdown: bool,
    #[serde(default)]
    pub target_blank: bool,
    #[serde(default)]
    pub include_vars: bool,
    #[serde(default)]
    pub keep_time: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Normalized index extracted from a [`DashboardDocument`] for search, authz, and impact queries (R1, §2.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardIndex {
    pub uid: String,
    pub title: String,
    pub slug: String,
    pub tags: Vec<String>,
    pub panel_count: usize,
    pub datasource_uids: Vec<String>,
    pub variable_names: Vec<String>,
    pub panel_types: Vec<String>,
    pub transformation_ids: Vec<String>,
    pub has_expressions: bool,
}

/// Converts a dashboard title into a Grafana-compatible URL slug.
#[must_use]
pub fn slugify_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "dashboard".to_owned()
    } else {
        trimmed.to_owned()
    }
}

impl DashboardDocument {
    /// Extracts the normalized [`DashboardIndex`] from this document (R1).
    #[must_use]
    pub fn extract_index(&self) -> DashboardIndex {
        let uid = self.uid.clone().unwrap_or_default();
        let title = self.title.clone().unwrap_or_default();
        let slug = slugify_title(&title);
        let tags = self.tags.clone().unwrap_or_default();

        let mut panel_count = 0usize;
        let mut ds_uids = BTreeSet::new();
        let mut panel_types = BTreeSet::new();
        let mut tx_ids = BTreeSet::new();
        let mut has_expressions = false;

        fn visit_panels(
            panels: &[PanelSpec],
            panel_count: &mut usize,
            ds_uids: &mut BTreeSet<String>,
            panel_types: &mut BTreeSet<String>,
            tx_ids: &mut BTreeSet<String>,
            has_expressions: &mut bool,
        ) {
            for p in panels {
                if p.panel_type == "row" {
                    if let Some(nested) = &p.panels {
                        visit_panels(
                            nested,
                            panel_count,
                            ds_uids,
                            panel_types,
                            tx_ids,
                            has_expressions,
                        );
                    }
                    continue;
                }
                *panel_count += 1;
                panel_types.insert(p.panel_type.clone());
                if let Some(ds) = &p.datasource
                    && let Some(u) = &ds.uid
                {
                    if u == "__expr__" || u == "-100" {
                        *has_expressions = true;
                    } else if !u.is_empty() && u != "-- Mixed --" {
                        ds_uids.insert(u.clone());
                    }
                }
                if let Some(targets) = &p.targets {
                    for t in targets {
                        if let Some(ds) = t.get("datasource").and_then(Value::as_object)
                            && let Some(u) = ds.get("uid").and_then(Value::as_str)
                        {
                            if u == "__expr__" || u == "-100" {
                                *has_expressions = true;
                            } else if !u.is_empty() && u != "-- Mixed --" {
                                ds_uids.insert(u.to_owned());
                            }
                        }
                    }
                }
                if let Some(txs) = &p.transformations {
                    for tx in txs {
                        tx_ids.insert(tx.id.clone());
                    }
                }
            }
        }

        if let Some(panels) = &self.panels {
            visit_panels(
                panels,
                &mut panel_count,
                &mut ds_uids,
                &mut panel_types,
                &mut tx_ids,
                &mut has_expressions,
            );
        }

        let variable_names = self
            .templating
            .as_ref()
            .map(|t| t.list.iter().map(|v| v.name.clone()).collect())
            .unwrap_or_default();

        if let Some(templating) = &self.templating {
            for v in &templating.list {
                if let Some(ds) = &v.datasource
                    && let Some(u) = &ds.uid
                    && !u.is_empty()
                    && u != "-- Mixed --"
                    && u != "__expr__"
                {
                    ds_uids.insert(u.clone());
                }
            }
        }

        DashboardIndex {
            uid,
            title,
            slug,
            tags,
            panel_count,
            datasource_uids: ds_uids.into_iter().collect(),
            variable_names,
            panel_types: panel_types.into_iter().collect(),
            transformation_ids: tx_ids.into_iter().collect(),
            has_expressions,
        }
    }

    /// Exports this dashboard at `schemaVersion: 42`, removing `id` and ordering all JSON keys
    /// deterministically for byte-stable export (R2, §7.2).
    #[must_use]
    pub fn to_export_value(&self) -> Value {
        let mut cloned = self.clone();
        cloned.id = None;
        cloned.schema_version = LATEST_SCHEMA_VERSION;
        let val = serde_json::to_value(&cloned).unwrap_or(Value::Null);
        CanonicalJson::canonicalize(&val)
    }
}

/// Recursively sorts JSON object keys into lexicographical order for byte-stable export (R2).
pub struct CanonicalJson;

impl CanonicalJson {
    #[must_use]
    pub fn canonicalize(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut sorted: BTreeMap<String, Value> = BTreeMap::new();
                for (k, v) in map {
                    sorted.insert(k.clone(), Self::canonicalize(v));
                }
                let mut out = Map::with_capacity(sorted.len());
                for (k, v) in sorted {
                    out.insert(k, v);
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(Self::canonicalize).collect()),
            other => other.clone(),
        }
    }

    /// Canonicalizes a dashboard JSON value for export: sets `schemaVersion = 42`, removes `id`,
    /// and sorts keys recursively.
    #[must_use]
    pub fn export_dashboard_json(raw: &Value) -> Value {
        let mut canonical = Self::canonicalize(raw);
        if let Value::Object(map) = &mut canonical {
            map.remove("id");
            map.insert(
                "schemaVersion".to_owned(),
                Value::from(LATEST_SCHEMA_VERSION),
            );
        }
        Self::canonicalize(&canonical)
    }
}
