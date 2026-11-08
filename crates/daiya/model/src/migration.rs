#![forbid(unsafe_code)]
//! Schema version migrations from `schemaVersion` 36 up to 42, plus v2beta1 -> v1 conversion,
//! ported from `GF apps/dashboard/pkg/migration/schemaversion/v36..v42.go` (R1, §6, §7.1, §8).

use crate::dashboard::CanonicalJson;
use crate::error::{CrError, CrResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub const MIN_SUPPORTED_SCHEMA_VERSION: u16 = 36;
pub const LATEST_SCHEMA_VERSION: u16 = 42;

/// Datasource metadata used during v36 migration (`datasource_utils.go`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataSourceInfo {
    pub uid: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ds_type: String,
    pub api_version: String,
    pub is_default: bool,
}

/// O(1) lookup index of datasources in an organization (`datasource_utils.go:DatasourceIndex`).
#[derive(Debug, Clone, Default)]
pub struct DatasourceIndex {
    by_name: HashMap<String, DataSourceInfo>,
    by_uid: HashMap<String, DataSourceInfo>,
    default_ds: Option<DataSourceInfo>,
}

impl DatasourceIndex {
    #[must_use]
    pub fn new(datasources: Vec<DataSourceInfo>) -> Self {
        let mut idx = Self {
            by_name: HashMap::with_capacity(datasources.len()),
            by_uid: HashMap::with_capacity(datasources.len()),
            default_ds: None,
        };
        for ds in datasources {
            if !ds.name.is_empty() {
                idx.by_name.insert(ds.name.clone(), ds.clone());
            }
            if !ds.uid.is_empty() {
                idx.by_uid.insert(ds.uid.clone(), ds.clone());
            }
            if ds.is_default {
                idx.default_ds = Some(ds);
            }
        }
        idx
    }

    #[must_use]
    pub fn lookup(&self, name_or_uid: &str) -> Option<&DataSourceInfo> {
        self.by_name
            .get(name_or_uid)
            .or_else(|| self.by_uid.get(name_or_uid))
    }

    #[must_use]
    pub fn lookup_by_uid(&self, uid: &str) -> Option<&DataSourceInfo> {
        self.by_uid.get(uid)
    }

    #[must_use]
    pub fn lookup_by_name(&self, name: &str) -> Option<&DataSourceInfo> {
        self.by_name.get(name)
    }

    #[must_use]
    pub fn get_default(&self) -> Option<&DataSourceInfo> {
        self.default_ds.as_ref()
    }
}

/// Builds a structured datasource reference object (`datasource_utils.go:GetDataSourceRef`).
#[must_use]
pub fn get_datasource_ref(ds: Option<&DataSourceInfo>) -> Value {
    let Some(ds) = ds else {
        return Value::Null;
    };
    let mut map = Map::new();
    map.insert("uid".to_owned(), Value::String(ds.uid.clone()));
    map.insert("type".to_owned(), Value::String(ds.ds_type.clone()));
    if !ds.api_version.is_empty() {
        map.insert(
            "apiVersion".to_owned(),
            Value::String(ds.api_version.clone()),
        );
    }
    Value::Object(map)
}

/// Checks if `val` is already a valid `DataSourceRef` object (has non-empty `uid` or `type`).
#[must_use]
pub fn is_datasource_ref(val: &Value) -> bool {
    let Some(obj) = val.as_object() else {
        return false;
    };
    let has_uid = obj
        .get("uid")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    let has_type = obj
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    has_uid || has_type
}

/// Converts a datasource name/uid string or null to a structured reference object
/// (`datasource_utils.go:MigrateDatasourceNameToRef`).
#[must_use]
pub fn migrate_datasource_name_to_ref(
    name_or_ref: Option<&Value>,
    return_default_as_null: bool,
    index: &DatasourceIndex,
) -> Value {
    let is_null_or_default = match name_or_ref {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) if s == "default" => true,
        _ => false,
    };
    if return_default_as_null && is_null_or_default {
        return Value::Null;
    }
    if let Some(v) = name_or_ref
        && is_datasource_ref(v)
    {
        return v.clone();
    }
    if is_null_or_default && let Some(ds) = index.get_default() {
        return get_datasource_ref(Some(ds));
    }
    if let Some(Value::String(s)) = name_or_ref {
        if s.is_empty() {
            return Value::Object(Map::new());
        }
        if let Some(ds) = index.lookup(s) {
            return get_datasource_ref(Some(ds));
        }
        return json!({ "uid": s });
    }
    Value::Null
}

/// Extracts `schemaVersion` from a dashboard JSON object (`migrations.go:GetSchemaVersion`).
#[must_use]
pub fn get_schema_version(dash: &Value) -> u16 {
    let Some(v) = dash.get("schemaVersion") else {
        return 0;
    };
    match v {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f as u64))
            .and_then(|u| u16::try_from(u).ok())
            .unwrap_or(0),
        Value::String(s) => s.parse::<u16>().unwrap_or(0),
        _ => 0,
    }
}

/// Report of schema migrations applied to a dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MigrationReport {
    pub original_schema_version: u16,
    pub final_schema_version: u16,
    pub steps_applied: Vec<u16>,
    pub converted_from_v2beta1: bool,
    pub warnings: Vec<String>,
}

/// V36 migration: converts legacy string/null datasource references in annotations, template
/// variables, panels, nested row panels, and targets into structured UID-based objects (`v36.go`).
pub fn migrate_v36(dashboard: &mut Value, index: &DatasourceIndex) {
    let Some(dash_obj) = dashboard.as_object_mut() else {
        return;
    };
    dash_obj.insert("schemaVersion".to_owned(), Value::from(36u16));

    // 1. migrateAnnotations
    if let Some(annotations) = dash_obj
        .get_mut("annotations")
        .and_then(Value::as_object_mut)
        && let Some(list) = annotations.get_mut("list").and_then(Value::as_array_mut)
    {
        for query in list {
            if let Some(q_map) = query.as_object_mut() {
                let migrated =
                    migrate_datasource_name_to_ref(q_map.get("datasource"), false, index);
                q_map.insert("datasource".to_owned(), migrated);
            }
        }
    }

    // 2. migrateTemplateVariables
    if let Some(templating) = dash_obj
        .get_mut("templating")
        .and_then(Value::as_object_mut)
        && let Some(list) = templating.get_mut("list").and_then(Value::as_array_mut)
    {
        let default_ds = index.get_default();
        for var in list {
            if let Some(var_map) = var.as_object_mut() {
                let var_type = var_map.get("type").and_then(Value::as_str).unwrap_or("");
                if var_type != "query" {
                    continue;
                }
                if matches!(var_map.get("datasource"), Some(Value::Null)) {
                    var_map.insert("datasource".to_owned(), get_datasource_ref(default_ds));
                }
            }
        }
    }

    // 3. migratePanels
    if let Some(panels) = dash_obj.get_mut("panels").and_then(Value::as_array_mut) {
        for panel in panels {
            if let Some(panel_map) = panel.as_object_mut() {
                migrate_panel_datasources_internal(panel_map, index, false);
                if let Some(nested) = panel_map.get_mut("panels").and_then(Value::as_array_mut) {
                    for np in nested {
                        if let Some(np_map) = np.as_object_mut() {
                            migrate_panel_datasources_internal(np_map, index, true);
                        }
                    }
                }
            }
        }
    }
}

fn migrate_panel_datasources_internal(
    panel_map: &mut Map<String, Value>,
    index: &DatasourceIndex,
    is_nested: bool,
) {
    let default_ds = index.get_default();
    let mut panel_ds_was_default = false;

    let has_non_empty_targets = panel_map
        .get("targets")
        .and_then(Value::as_array)
        .is_some_and(|t| !t.is_empty());

    if !has_non_empty_targets {
        if !is_nested {
            panel_map.insert("targets".to_owned(), json!([{ "refId": "A" }]));
        } else {
            return;
        }
    }

    let targets_len = panel_map
        .get("targets")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);

    match panel_map.get("datasource") {
        None | Some(Value::Null) => {
            if targets_len > 0 {
                panel_map.insert("datasource".to_owned(), get_datasource_ref(default_ds));
                panel_ds_was_default = true;
            }
        }
        Some(ds) => {
            let is_empty_obj = ds.as_object().is_some_and(Map::is_empty);
            if !is_empty_obj {
                let migrated = migrate_datasource_name_to_ref(Some(ds), false, index);
                panel_map.insert("datasource".to_owned(), migrated);
            }
        }
    }

    let mut updated_panel_ds: Option<Value> = None;
    let current_panel_ds = panel_map.get("datasource").cloned();

    if let Some(targets) = panel_map.get_mut("targets").and_then(Value::as_array_mut) {
        for target in targets {
            let Some(target_map) = target.as_object_mut() else {
                continue;
            };
            let target_ds = target_map.get("datasource");
            let needs_default = match target_ds {
                None | Some(Value::Null) => true,
                Some(Value::Object(m)) => {
                    matches!(m.get("uid"), None | Some(Value::Null))
                }
                _ => false,
            };

            if needs_default {
                let effective_panel_ds = updated_panel_ds.as_ref().or(current_panel_ds.as_ref());
                if let Some(Value::Object(p_ds)) = effective_panel_ds {
                    let uid = p_ds.get("uid").and_then(Value::as_str).unwrap_or("");
                    if uid != "-- Mixed --" {
                        target_map.insert("datasource".to_owned(), Value::Object(p_ds.clone()));
                    } else {
                        let migrated = migrate_datasource_name_to_ref(target_ds, false, index);
                        target_map.insert("datasource".to_owned(), migrated);
                    }
                }
            } else {
                let migrated = migrate_datasource_name_to_ref(target_ds, false, index);
                target_map.insert("datasource".to_owned(), migrated);
            }

            if panel_ds_was_default && let Some(Value::Object(t_ds)) = target_map.get("datasource")
            {
                let uid = t_ds.get("uid").and_then(Value::as_str).unwrap_or("");
                if !uid.is_empty() && uid != "__expr__" {
                    updated_panel_ds = Some(Value::Object(t_ds.clone()));
                }
            }
        }
    }

    if let Some(new_p_ds) = updated_panel_ds {
        panel_map.insert("datasource".to_owned(), new_p_ds);
    }
}

/// V37 migration: normalizes `options.legend` to use `showLegend` consistently (`v37.go`).
pub fn migrate_v37(dashboard: &mut Value) {
    let Some(dash_obj) = dashboard.as_object_mut() else {
        return;
    };
    dash_obj.insert("schemaVersion".to_owned(), Value::from(37u16));
    if let Some(panels) = dash_obj.get_mut("panels").and_then(Value::as_array_mut) {
        process_panels_v37(panels);
    }
}

fn process_panels_v37(panels: &mut [Value]) {
    for panel in panels {
        let Some(p) = panel.as_object_mut() else {
            continue;
        };
        if p.get("type").and_then(Value::as_str) == Some("row") {
            if let Some(nested) = p.get_mut("panels").and_then(Value::as_array_mut) {
                process_panels_v37(nested);
            }
            continue;
        }
        let Some(options) = p.get_mut("options").and_then(Value::as_object_mut) else {
            continue;
        };
        let Some(legend) = options.get_mut("legend").and_then(Value::as_object_mut) else {
            continue;
        };
        let display_mode = legend
            .get("displayMode")
            .and_then(Value::as_str)
            .unwrap_or("");
        let show_legend = legend.get("showLegend").and_then(Value::as_bool);

        if display_mode == "hidden" || show_legend == Some(false) {
            legend.insert("displayMode".to_owned(), Value::String("list".to_owned()));
            legend.insert("showLegend".to_owned(), Value::Bool(false));
        } else {
            legend.insert("showLegend".to_owned(), Value::Bool(true));
        }
    }
}

/// V38 migration: migrates table panel `displayMode` to structured `cellOptions` (`v38.go`).
pub fn migrate_v38(dashboard: &mut Value) {
    let Some(dash_obj) = dashboard.as_object_mut() else {
        return;
    };
    dash_obj.insert("schemaVersion".to_owned(), Value::from(38u16));
    if let Some(panels) = dash_obj.get_mut("panels").and_then(Value::as_array_mut) {
        process_panels_v38(panels);
    }
}

fn process_panels_v38(panels: &mut [Value]) {
    for panel in panels {
        let Some(p) = panel.as_object_mut() else {
            continue;
        };
        let p_type = p.get("type").and_then(Value::as_str).unwrap_or("");
        if p_type == "row" {
            if let Some(nested) = p.get_mut("panels").and_then(Value::as_array_mut) {
                process_panels_v38(nested);
            }
            continue;
        }
        if p_type != "table" {
            continue;
        }
        let Some(field_config) = p.get_mut("fieldConfig").and_then(Value::as_object_mut) else {
            continue;
        };
        if let Some(defaults) = field_config
            .get_mut("defaults")
            .and_then(Value::as_object_mut)
            && let Some(custom) = defaults.get_mut("custom").and_then(Value::as_object_mut)
            && let Some(dm_str) = custom
                .get("displayMode")
                .and_then(Value::as_str)
                .map(str::to_owned)
        {
            // Grafana migrates only a string displayMode; anything else stays verbatim.
            custom.remove("displayMode");
            custom.insert(
                "cellOptions".to_owned(),
                migrate_table_display_mode_to_cell_options(&dm_str),
            );
        }
        if let Some(overrides) = field_config
            .get_mut("overrides")
            .and_then(Value::as_array_mut)
        {
            for ov in overrides {
                let Some(ov_map) = ov.as_object_mut() else {
                    continue;
                };
                let Some(props) = ov_map.get_mut("properties").and_then(Value::as_array_mut) else {
                    continue;
                };
                for prop in props {
                    let Some(prop_map) = prop.as_object_mut() else {
                        continue;
                    };
                    if prop_map.get("id").and_then(Value::as_str) == Some("custom.displayMode") {
                        prop_map.insert(
                            "id".to_owned(),
                            Value::String("custom.cellOptions".to_owned()),
                        );
                        match prop_map.get("value").cloned() {
                            Some(Value::String(s)) => {
                                prop_map.insert(
                                    "value".to_owned(),
                                    migrate_table_display_mode_to_cell_options(&s),
                                );
                            }
                            None => {
                                prop_map.insert("value".to_owned(), Value::Object(Map::new()));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

fn migrate_table_display_mode_to_cell_options(display_mode: &str) -> Value {
    match display_mode {
        "basic" | "gradient-gauge" | "lcd-gauge" => {
            let gauge_mode = match display_mode {
                "gradient-gauge" => "gradient",
                "lcd-gauge" => "lcd",
                _ => "basic",
            };
            json!({
                "type": "gauge",
                "mode": gauge_mode,
            })
        }
        "color-background" | "color-background-solid" => {
            let mode = if display_mode == "color-background" {
                "gradient"
            } else {
                "basic"
            };
            json!({
                "type": "color-background",
                "mode": mode,
            })
        }
        other => json!({
            "type": other,
        }),
    }
}

/// V39 migration: migrates `timeSeriesTable` transformation `refIdToStat` options (`v39.go`).
pub fn migrate_v39(dashboard: &mut Value) {
    let Some(dash_obj) = dashboard.as_object_mut() else {
        return;
    };
    dash_obj.insert("schemaVersion".to_owned(), Value::from(39u16));
    if let Some(panels) = dash_obj.get_mut("panels").and_then(Value::as_array_mut) {
        process_panels_v39(panels);
    }
}

fn process_panels_v39(panels: &mut [Value]) {
    for panel in panels {
        let Some(p) = panel.as_object_mut() else {
            continue;
        };
        if p.get("type").and_then(Value::as_str) == Some("row") {
            if let Some(nested) = p.get_mut("panels").and_then(Value::as_array_mut) {
                process_panels_v39(nested);
            }
            continue;
        }
        let Some(transformations) = p.get_mut("transformations").and_then(Value::as_array_mut)
        else {
            continue;
        };
        for tx in transformations {
            let Some(t) = tx.as_object_mut() else {
                continue;
            };
            if t.get("id").and_then(Value::as_str) != Some("timeSeriesTable") {
                continue;
            }
            let Some(options) = t.get("options").and_then(Value::as_object) else {
                continue;
            };
            let Some(ref_id_stats) = options.get("refIdToStat").and_then(Value::as_object) else {
                continue;
            };
            let mut new_options = Map::new();
            for (ref_id, stat) in ref_id_stats {
                new_options.insert(ref_id.clone(), json!({ "stat": stat }));
            }
            t.insert("options".to_owned(), Value::Object(new_options));
        }
    }
}

/// V40 migration: normalizes dashboard `refresh` to string (`v40.go`).
pub fn migrate_v40(dashboard: &mut Value) {
    let Some(dash_obj) = dashboard.as_object_mut() else {
        return;
    };
    dash_obj.insert("schemaVersion".to_owned(), Value::from(40u16));
    let refresh_str = dash_obj
        .get("refresh")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    dash_obj.insert("refresh".to_owned(), Value::String(refresh_str));
}

/// V41 migration: removes deprecated `timepicker.time_options` (`v41.go`).
pub fn migrate_v41(dashboard: &mut Value) {
    let Some(dash_obj) = dashboard.as_object_mut() else {
        return;
    };
    dash_obj.insert("schemaVersion".to_owned(), Value::from(41u16));
    if let Some(timepicker) = dash_obj
        .get_mut("timepicker")
        .and_then(Value::as_object_mut)
    {
        timepicker.remove("time_options");
    }
}

/// V42 migration: sets `hideFrom.tooltip = true` whenever `hideFrom.viz == true` in overrides (`v42.go`).
pub fn migrate_v42(dashboard: &mut Value) {
    let Some(dash_obj) = dashboard.as_object_mut() else {
        return;
    };
    dash_obj.insert("schemaVersion".to_owned(), Value::from(42u16));
    if let Some(panels) = dash_obj.get_mut("panels").and_then(Value::as_array_mut) {
        for panel in panels {
            if let Some(p) = panel.as_object_mut() {
                migrate_hide_from_for_panel(p);
            }
        }
    }
}

fn migrate_hide_from_for_panel(panel: &mut Map<String, Value>) {
    if let Some(field_config) = panel.get_mut("fieldConfig").and_then(Value::as_object_mut)
        && let Some(overrides) = field_config
            .get_mut("overrides")
            .and_then(Value::as_array_mut)
    {
        for ov in overrides {
            let Some(ov_map) = ov.as_object_mut() else {
                continue;
            };
            let Some(props) = ov_map.get_mut("properties").and_then(Value::as_array_mut) else {
                continue;
            };
            for prop in props {
                let Some(prop_map) = prop.as_object_mut() else {
                    continue;
                };
                if prop_map.get("id").and_then(Value::as_str) == Some("custom.hideFrom")
                    && let Some(val_map) = prop_map.get_mut("value").and_then(Value::as_object_mut)
                    && val_map.get("viz").and_then(Value::as_bool).unwrap_or(false)
                {
                    val_map.insert("tooltip".to_owned(), Value::Bool(true));
                }
            }
        }
    }
    if let Some(nested) = panel.get_mut("panels").and_then(Value::as_array_mut) {
        for np in nested {
            if let Some(np_map) = np.as_object_mut() {
                migrate_hide_from_for_panel(np_map);
            }
        }
    }
}

/// Detects whether `doc` is a Grafana v2beta1 dashboard (`elements` + `layout` or `apiVersion` with `spec.elements`).
#[must_use]
pub fn is_v2beta1_dashboard(doc: &Value) -> bool {
    let Some(obj) = doc.as_object() else {
        return false;
    };
    if obj.contains_key("elements") && obj.contains_key("layout") {
        return true;
    }
    if let Some(spec) = obj.get("spec").and_then(Value::as_object)
        && spec.contains_key("elements")
        && spec.contains_key("layout")
    {
        return true;
    }
    false
}

/// Converts a Grafana v2alpha1/v2beta1 dashboard specification into a classic v1 dashboard at `schemaVersion: 42` (§7.1).
pub fn convert_v2beta1_to_v1(doc: &Value) -> CrResult<Value> {
    let root = doc
        .as_object()
        .ok_or_else(|| CrError::BadRequest("v2beta1 dashboard must be a JSON object".into()))?;
    let spec = root.get("spec").and_then(Value::as_object).unwrap_or(root);
    let metadata = root.get("metadata").and_then(Value::as_object);

    let uid = metadata
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .or_else(|| spec.get("uid").and_then(Value::as_str))
        .unwrap_or("converted-v2");
    let title = spec
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Converted Dashboard");
    let description = spec.get("description").and_then(Value::as_str);
    let editable = spec
        .get("editable")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let tags = spec
        .get("tags")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));

    let graph_tooltip = match spec.get("cursorSync").and_then(Value::as_str) {
        Some("Crosshair") => 1u8,
        Some("Tooltip") => 2u8,
        _ => 0u8,
    };

    let time_settings = spec.get("timeSettings").and_then(Value::as_object);
    let from = time_settings
        .and_then(|ts| ts.get("from"))
        .and_then(Value::as_str)
        .unwrap_or("now-6h");
    let to = time_settings
        .and_then(|ts| ts.get("to"))
        .and_then(Value::as_str)
        .unwrap_or("now");
    let timezone = time_settings
        .and_then(|ts| ts.get("timezone"))
        .and_then(Value::as_str)
        .unwrap_or("browser");
    let refresh = time_settings
        .and_then(|ts| ts.get("autoRefresh"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let elements = spec
        .get("elements")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    // Map layout items to gridPos by element name
    let mut grid_positions: HashMap<String, Value> = HashMap::new();
    if let Some(layout) = spec.get("layout").and_then(Value::as_object)
        && let Some(l_spec) = layout.get("spec").and_then(Value::as_object)
        && let Some(items) = l_spec.get("items").and_then(Value::as_array)
    {
        for (idx, item) in items.iter().enumerate() {
            if let Some(i_spec) = item.get("spec").and_then(Value::as_object) {
                let el_name = i_spec
                    .get("element")
                    .and_then(|e| e.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let x = i_spec.get("x").and_then(Value::as_u64).unwrap_or(0);
                let y = i_spec
                    .get("y")
                    .and_then(Value::as_u64)
                    .unwrap_or((idx as u64) * 8);
                let w = i_spec.get("width").and_then(Value::as_u64).unwrap_or(12);
                let h = i_spec.get("height").and_then(Value::as_u64).unwrap_or(8);
                grid_positions.insert(
                    el_name.to_owned(),
                    json!({ "x": x, "y": y, "w": w, "h": h }),
                );
            }
        }
    }

    let mut panels = Vec::new();
    for (el_key, el_val) in &elements {
        let Some(el_obj) = el_val.as_object() else {
            continue;
        };
        let kind = el_obj
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("Panel");
        let Some(el_spec) = el_obj.get("spec").and_then(Value::as_object) else {
            continue;
        };
        let id = el_spec.get("id").and_then(Value::as_u64).unwrap_or(1);
        let p_title = el_spec.get("title").and_then(Value::as_str).unwrap_or("");
        let grid_pos = grid_positions
            .get(el_key)
            .cloned()
            .unwrap_or_else(|| json!({ "x": 0, "y": 0, "w": 12, "h": 8 }));

        if kind == "LibraryPanel" {
            let lib_ref = el_spec.get("libraryPanel").cloned().unwrap_or(Value::Null);
            panels.push(json!({
                "id": id,
                "title": p_title,
                "type": "library-panel-ref",
                "gridPos": grid_pos,
                "libraryPanel": lib_ref,
            }));
            continue;
        }

        let viz = el_spec.get("vizConfig").and_then(Value::as_object);
        let panel_type = viz
            .and_then(|v| v.get("group").or_else(|| v.get("kind")))
            .and_then(Value::as_str)
            .unwrap_or("timeseries");
        let viz_spec = viz.and_then(|v| v.get("spec")).and_then(Value::as_object);
        let options = viz_spec
            .and_then(|vs| vs.get("options"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let field_config = viz_spec
            .and_then(|vs| vs.get("fieldConfig"))
            .cloned()
            .unwrap_or_else(|| json!({ "defaults": {}, "overrides": [] }));

        let mut targets = Vec::new();
        let mut transformations = Vec::new();
        if let Some(data_spec) = el_spec
            .get("data")
            .and_then(|d| d.get("spec"))
            .and_then(Value::as_object)
        {
            if let Some(queries) = data_spec.get("queries").and_then(Value::as_array) {
                for q_item in queries {
                    if let Some(pq_spec) = q_item.get("spec").and_then(Value::as_object) {
                        let ref_id = pq_spec.get("refId").and_then(Value::as_str).unwrap_or("A");
                        let hidden = pq_spec
                            .get("hidden")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        if let Some(dq) = pq_spec.get("query").and_then(Value::as_object) {
                            let group = dq
                                .get("group")
                                .or_else(|| dq.get("kind"))
                                .and_then(Value::as_str)
                                .unwrap_or("prometheus");
                            let ds_uid = dq
                                .get("datasource")
                                .and_then(|d| d.get("name").or_else(|| d.get("uid")))
                                .and_then(Value::as_str)
                                .or_else(|| {
                                    pq_spec
                                        .get("datasource")
                                        .and_then(|d| d.get("uid"))
                                        .and_then(Value::as_str)
                                })
                                .unwrap_or(group);
                            let mut target_map = dq
                                .get("spec")
                                .and_then(Value::as_object)
                                .cloned()
                                .unwrap_or_default();
                            target_map.insert("refId".to_owned(), Value::String(ref_id.to_owned()));
                            if hidden {
                                target_map.insert("hide".to_owned(), Value::Bool(true));
                            }
                            target_map.insert(
                                "datasource".to_owned(),
                                json!({ "type": group, "uid": ds_uid }),
                            );
                            targets.push(Value::Object(target_map));
                        }
                    }
                }
            }
            if let Some(txs) = data_spec.get("transformations").and_then(Value::as_array) {
                for tx_item in txs {
                    if let Some(tx_spec) = tx_item.get("spec") {
                        transformations.push(tx_spec.clone());
                    }
                }
            }
        }

        let mut panel_obj = json!({
            "id": id,
            "title": p_title,
            "type": panel_type,
            "gridPos": grid_pos,
            "targets": targets,
            "options": options,
            "fieldConfig": field_config,
        });
        if !transformations.is_empty()
            && let Some(po) = panel_obj.as_object_mut()
        {
            po.insert("transformations".to_owned(), Value::Array(transformations));
        }
        panels.push(panel_obj);
    }

    let mut out = json!({
        "uid": uid,
        "title": title,
        "tags": tags,
        "editable": editable,
        "graphTooltip": graph_tooltip,
        "timezone": timezone,
        "refresh": refresh,
        "schemaVersion": LATEST_SCHEMA_VERSION,
        "time": { "from": from, "to": to },
        "panels": panels,
    });
    if let Some(desc) = description
        && let Some(m) = out.as_object_mut()
    {
        m.insert("description".to_owned(), Value::String(desc.to_owned()));
    }
    Ok(out)
}

/// Runs the full migration chain to `schemaVersion: 42` (R1).
///
/// - Converts v2beta1 documents to v1 first if needed.
/// - If `schemaVersion < 36`, records the named warning `"schemaVersion below 36: migrate in Grafana first"`
///   and runs steps 36..=42.
/// - For `schemaVersion` in 36..42, runs each subsequent step `v + 1 ..= 42`.
pub fn migrate_to_latest(
    dashboard: &mut Value,
    ds_index: &DatasourceIndex,
) -> CrResult<MigrationReport> {
    if !dashboard.is_object() {
        return Err(CrError::MigrationFailed(
            "dashboard JSON must be an object".into(),
        ));
    }

    let mut converted_from_v2beta1 = false;
    let mut warnings = Vec::new();

    if is_v2beta1_dashboard(dashboard) {
        *dashboard = convert_v2beta1_to_v1(dashboard)?;
        converted_from_v2beta1 = true;
        warnings.push("dashboard v2beta1 converted to classic v1 at schemaVersion 42".to_owned());
    }

    let original_version = get_schema_version(dashboard);
    let mut steps_applied = Vec::new();

    let start_from = if original_version < MIN_SUPPORTED_SCHEMA_VERSION {
        warnings.push("schemaVersion below 36: migrate in Grafana first".to_owned());
        MIN_SUPPORTED_SCHEMA_VERSION
    } else {
        original_version + 1
    };

    for step in start_from..=LATEST_SCHEMA_VERSION {
        match step {
            36 => migrate_v36(dashboard, ds_index),
            37 => migrate_v37(dashboard),
            38 => migrate_v38(dashboard),
            39 => migrate_v39(dashboard),
            40 => migrate_v40(dashboard),
            41 => migrate_v41(dashboard),
            42 => migrate_v42(dashboard),
            _ => {}
        }
        steps_applied.push(step);
    }

    // Ensure canonical schemaVersion 42 even if original was >= 42
    if let Some(obj) = dashboard.as_object_mut() {
        obj.insert(
            "schemaVersion".to_owned(),
            Value::from(LATEST_SCHEMA_VERSION),
        );
    }
    *dashboard = CanonicalJson::canonicalize(dashboard);

    Ok(MigrationReport {
        original_schema_version: original_version,
        final_schema_version: LATEST_SCHEMA_VERSION,
        steps_applied,
        converted_from_v2beta1,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ds_index() -> DatasourceIndex {
        DatasourceIndex::new(vec![
            DataSourceInfo {
                uid: "prometheus-uid-123".into(),
                name: "prometheus-prod".into(),
                ds_type: "prometheus".into(),
                api_version: "v1".into(),
                is_default: true,
            },
            DataSourceInfo {
                uid: "cloudwatch-uid-456".into(),
                name: "CloudWatch".into(),
                ds_type: "cloudwatch".into(),
                api_version: "v1".into(),
                is_default: false,
            },
        ])
    }

    #[test]
    fn step_v36_migrates_datasource_strings_and_nulls_to_refs() {
        let index = test_ds_index();
        let mut dash = json!({
            "schemaVersion": 35,
            "annotations": {
                "list": [
                    { "name": "Deploy", "datasource": "prometheus-prod" }
                ]
            },
            "templating": {
                "list": [
                    { "name": "env", "type": "query", "datasource": null }
                ]
            },
            "panels": [
                {
                    "id": 1,
                    "type": "timeseries",
                    "datasource": "CloudWatch",
                    "targets": [
                        { "refId": "A", "datasource": null }
                    ]
                }
            ]
        });

        migrate_v36(&mut dash, &index);
        assert_eq!(dash["schemaVersion"], 36);
        assert_eq!(
            dash["annotations"]["list"][0]["datasource"],
            json!({
                "uid": "prometheus-uid-123",
                "type": "prometheus",
                "apiVersion": "v1"
            })
        );
        assert_eq!(
            dash["templating"]["list"][0]["datasource"],
            json!({
                "uid": "prometheus-uid-123",
                "type": "prometheus",
                "apiVersion": "v1"
            })
        );
        assert_eq!(
            dash["panels"][0]["datasource"],
            json!({
                "uid": "cloudwatch-uid-456",
                "type": "cloudwatch",
                "apiVersion": "v1"
            })
        );
        assert_eq!(
            dash["panels"][0]["targets"][0]["datasource"],
            json!({
                "uid": "cloudwatch-uid-456",
                "type": "cloudwatch",
                "apiVersion": "v1"
            })
        );
    }

    #[test]
    fn step_v37_normalizes_legend_show_legend() {
        let mut dash = json!({
            "schemaVersion": 36,
            "panels": [
                {
                    "id": 1,
                    "type": "timeseries",
                    "options": {
                        "legend": {
                            "displayMode": "hidden",
                            "placement": "bottom"
                        }
                    }
                },
                {
                    "id": 2,
                    "type": "timeseries",
                    "options": {
                        "legend": {
                            "displayMode": "table",
                            "showLegend": false
                        }
                    }
                },
                {
                    "id": 3,
                    "type": "timeseries",
                    "options": {
                        "legend": {
                            "displayMode": "table",
                            "placement": "bottom"
                        }
                    }
                }
            ]
        });

        migrate_v37(&mut dash);
        assert_eq!(dash["schemaVersion"], 37);
        assert_eq!(
            dash["panels"][0]["options"]["legend"],
            json!({
                "displayMode": "list",
                "showLegend": false,
                "placement": "bottom"
            })
        );
        assert_eq!(
            dash["panels"][1]["options"]["legend"],
            json!({
                "displayMode": "list",
                "showLegend": false
            })
        );
        assert_eq!(
            dash["panels"][2]["options"]["legend"],
            json!({
                "displayMode": "table",
                "placement": "bottom",
                "showLegend": true
            })
        );
    }

    #[test]
    fn step_v38_migrates_table_display_mode_to_cell_options() {
        let mut dash = json!({
            "schemaVersion": 37,
            "panels": [
                {
                    "id": 1,
                    "type": "table",
                    "fieldConfig": {
                        "defaults": {
                            "custom": {
                                "displayMode": "gradient-gauge"
                            }
                        },
                        "overrides": [
                            {
                                "matcher": { "id": "byName", "options": "CPU" },
                                "properties": [
                                    {
                                        "id": "custom.displayMode",
                                        "value": "color-background-solid"
                                    }
                                ]
                            }
                        ]
                    }
                }
            ]
        });

        migrate_v38(&mut dash);
        assert_eq!(dash["schemaVersion"], 38);
        assert_eq!(
            dash["panels"][0]["fieldConfig"]["defaults"]["custom"],
            json!({
                "cellOptions": {
                    "type": "gauge",
                    "mode": "gradient"
                }
            })
        );
        assert_eq!(
            dash["panels"][0]["fieldConfig"]["overrides"][0]["properties"][0],
            json!({
                "id": "custom.cellOptions",
                "value": {
                    "type": "color-background",
                    "mode": "basic"
                }
            })
        );
    }

    #[test]
    fn step_v39_migrates_timeseries_table_ref_id_to_stat() {
        let mut dash = json!({
            "schemaVersion": 38,
            "panels": [
                {
                    "id": 1,
                    "type": "table",
                    "transformations": [
                        {
                            "id": "timeSeriesTable",
                            "options": {
                                "refIdToStat": {
                                    "A": "mean",
                                    "B": "max",
                                    "C": "last"
                                }
                            }
                        }
                    ]
                }
            ]
        });

        migrate_v39(&mut dash);
        assert_eq!(dash["schemaVersion"], 39);
        assert_eq!(
            dash["panels"][0]["transformations"][0]["options"],
            json!({
                "A": { "stat": "mean" },
                "B": { "stat": "max" },
                "C": { "stat": "last" }
            })
        );
    }

    #[test]
    fn step_v40_normalizes_refresh_to_string() {
        for bad in [json!(true), json!(30), json!(null)] {
            let mut dash = json!({ "schemaVersion": 39, "refresh": bad });
            migrate_v40(&mut dash);
            assert_eq!(dash["schemaVersion"], 40);
            assert_eq!(dash["refresh"], json!(""));
        }
        let mut valid = json!({ "schemaVersion": 39, "refresh": "1m" });
        migrate_v40(&mut valid);
        assert_eq!(valid["refresh"], json!("1m"));
    }

    #[test]
    fn step_v41_removes_timepicker_time_options() {
        let mut dash = json!({
            "schemaVersion": 40,
            "timepicker": {
                "refresh_intervals": ["5s", "10s", "30s", "1m"],
                "time_options": ["5m", "15m", "1h", "6h", "12h", "24h"]
            }
        });
        migrate_v41(&mut dash);
        assert_eq!(dash["schemaVersion"], 41);
        assert_eq!(
            dash["timepicker"],
            json!({
                "refresh_intervals": ["5s", "10s", "30s", "1m"]
            })
        );
    }

    #[test]
    fn step_v42_sets_hide_from_tooltip_when_viz_true() {
        let mut dash = json!({
            "schemaVersion": 41,
            "panels": [
                {
                    "id": 1,
                    "type": "timeseries",
                    "fieldConfig": {
                        "overrides": [
                            {
                                "properties": [
                                    {
                                        "id": "custom.hideFrom",
                                        "value": { "viz": true }
                                    }
                                ]
                            }
                        ]
                    }
                }
            ]
        });
        migrate_v42(&mut dash);
        assert_eq!(dash["schemaVersion"], 42);
        assert_eq!(
            dash["panels"][0]["fieldConfig"]["overrides"][0]["properties"][0]["value"],
            json!({ "viz": true, "tooltip": true })
        );
    }

    #[test]
    fn below_v36_emits_named_warning_and_migrates_to_42() {
        let mut dash = json!({
            "uid": "old-dash",
            "title": "Legacy",
            "schemaVersion": 30,
            "panels": []
        });
        let report = migrate_to_latest(&mut dash, &DatasourceIndex::default()).expect("migrated");
        assert_eq!(report.original_schema_version, 30);
        assert_eq!(report.final_schema_version, 42);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w == "schemaVersion below 36: migrate in Grafana first")
        );
    }

    #[test]
    fn v2beta1_converts_to_v1_at_42() {
        let mut v2 = json!({
            "apiVersion": "dashboard.grafana.app/v2beta1",
            "kind": "Dashboard",
            "metadata": { "name": "v2-uid" },
            "spec": {
                "title": "V2 Dashboard",
                "cursorSync": "Crosshair",
                "timeSettings": {
                    "from": "now-1h",
                    "to": "now",
                    "autoRefresh": "10s"
                },
                "elements": {
                    "panel-1": {
                        "kind": "Panel",
                        "spec": {
                            "id": 1,
                            "title": "Requests",
                            "vizConfig": {
                                "kind": "VizConfig",
                                "group": "timeseries",
                                "version": "1",
                                "spec": {
                                    "options": {},
                                    "fieldConfig": { "defaults": { "unit": "reqps" }, "overrides": [] }
                                }
                            },
                            "data": {
                                "kind": "QueryGroup",
                                "spec": {
                                    "queries": [
                                        {
                                            "kind": "PanelQuery",
                                            "spec": {
                                                "refId": "A",
                                                "hidden": false,
                                                "query": {
                                                    "kind": "DataQuery",
                                                    "group": "prometheus",
                                                    "datasource": { "name": "prometheus" },
                                                    "spec": { "expr": "up" }
                                                }
                                            }
                                        }
                                    ],
                                    "transformations": [],
                                    "queryOptions": {}
                                }
                            }
                        }
                    }
                },
                "layout": {
                    "kind": "GridLayout",
                    "spec": {
                        "items": [
                            {
                                "kind": "GridLayoutItem",
                                "spec": {
                                    "x": 0,
                                    "y": 0,
                                    "width": 12,
                                    "height": 8,
                                    "element": { "kind": "ElementReference", "name": "panel-1" }
                                }
                            }
                        ]
                    }
                }
            }
        });

        let report = migrate_to_latest(&mut v2, &DatasourceIndex::default()).expect("v2 migrated");
        assert!(report.converted_from_v2beta1);
        assert_eq!(v2["uid"], "v2-uid");
        assert_eq!(v2["schemaVersion"], 42);
        assert_eq!(v2["panels"][0]["type"], "timeseries");
        assert_eq!(v2["panels"][0]["targets"][0]["expr"], "up");
    }
}
