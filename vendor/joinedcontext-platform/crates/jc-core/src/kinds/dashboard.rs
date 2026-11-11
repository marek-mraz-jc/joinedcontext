//! `kind: Dashboard` and `kind: Layer`: the declarative dashboards of the Portal (T-0528,
//! UI-17, UI-18, MF-09, Architecture/10 §1).
//!
//! A dashboard is pages of layers and widgets; a layer is one Endpoint's entities of one type
//! with a style, its encodings and a static filter. Whether a public dashboard reads only
//! through public Endpoints (UI-19) is a cross-manifest check the Portal makes: a Layer alone
//! does not know its Endpoint's audience.

use crate::envelope::{Kind, ObjectMeta, Scope};
use crate::error::{Error, Result};
use crate::i18n::Text;
use crate::kinds::grid::GridConfig;
use crate::kinds::visualization::{self, VisualizationData};
use crate::names;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who may open the dashboard (UI-19); `private` when undeclared.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DashboardVisibility {
    /// Its author.
    #[default]
    Private,
    /// The members of its project.
    Project,
    /// Every signed-in user of the organization.
    Organization,
    /// Anyone; every layer must then read through an `audience: public` Endpoint.
    Public,
}

/// A widget of an analytics page (Architecture/10 §1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Widget {
    /// The widget, e.g. `temporal-chart` or `grid`.
    pub widget_type: String,
    /// The Endpoint it reads through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_ref: Option<String>,
    /// The entity it shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    /// The property it shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    /// The entity type a `grid` widget shows, the type decides its columns (UI-71), or the
    /// type a `bar-chart` or `histogram` counts (UI-93).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    /// The NGSI-LD `q` a `bar-chart` or `histogram` reads the type with: the explorer's filter
    /// when the chart came from there (UI-93).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// The configuration of a `grid` widget: the same object the explorer and a generated
    /// application are configured by, without the source and the type those two fields decide
    /// (SDK-30, T-1440).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<GridConfig>,
    /// The rows a `vega-lite` widget draws: one source of its Endpoint, which the platform reads
    /// and fills into the spec's dataset `source` (ADR-N-053 §2.2), never a URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<VisualizationData>,
    /// A `vega-lite` widget's Vega-Lite 5 specification: data, never code. It reads
    /// `{"data": {"name": "source"}}` and holds no expression beyond [`EXPRESSION_FUNCTIONS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, Value>>")]
    pub spec: Option<Value>,
}

/// The widget type of the entity grid.
pub const GRID_WIDGET: &str = "grid";

/// The widget types that count an entity type's values (UI-93): how many entities hold each value,
/// and how a number spreads.
pub const TYPE_CHARTS: [&str; 2] = ["bar-chart", "histogram"];

/// The longest `q` a chart widget carries, as long as a filter the explorer composes.
const MAX_CHART_Q: usize = 1024;

/// The widget type of a Vega-Lite chart over one source of its Endpoint (T-6786).
pub const VEGA_LITE_WIDGET: &str = "vega-lite";

/// The functions and constants a `vega-lite` widget's expressions may call: arithmetic, type
/// tests and conversions, dates and text. Nothing that reads a signal, the view, the event or
/// the window, and nothing that loads (T-6786).
pub const EXPRESSION_FUNCTIONS: &[&str] = &[
    "datum",
    "if",
    "true",
    "false",
    "null",
    "NaN",
    "Infinity",
    "PI",
    "E",
    "abs",
    "ceil",
    "floor",
    "round",
    "sqrt",
    "log",
    "exp",
    "pow",
    "min",
    "max",
    "clamp",
    "isNaN",
    "isFinite",
    "isValid",
    "isNumber",
    "isString",
    "isDate",
    "isBoolean",
    "toNumber",
    "toString",
    "toBoolean",
    "toDate",
    "datetime",
    "date",
    "day",
    "year",
    "quarter",
    "month",
    "hours",
    "minutes",
    "seconds",
    "utcdate",
    "utcday",
    "utcyear",
    "utcquarter",
    "utcmonth",
    "utchours",
    "utcminutes",
    "utcseconds",
    "timeFormat",
    "utcFormat",
    "format",
    "upper",
    "lower",
    "length",
    "substring",
    "indexOf",
    "lastIndexOf",
    "replace",
    "trim",
    "pad",
    "truncate",
    "join",
    "inrange",
    "span",
];

/// The longest expression a `vega-lite` widget carries.
const MAX_EXPRESSION: usize = 1024;

/// One page: a map of layers, or a grid of widgets, or both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Page {
    /// Shown as the page's tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `full-map`, `grid-2x2`, …; the Portal's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// Names of `Layer` manifests of the same project, drawn bottom to top.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<String>,
    /// The widgets of the page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub widgets: Vec<Widget>,
}

/// `spec` of a Dashboard (UI-17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DashboardSpec {
    /// The title (UI-50): one string, or the legacy map per locale (PF-24).
    pub title: Text,
    /// Who may open it.
    #[serde(default)]
    pub visibility: DashboardVisibility,
    /// At least one page.
    pub pages: Vec<Page>,
}

impl Kind for DashboardSpec {
    const KIND: &'static str = "Dashboard";
    const PLURAL: &'static str = "dashboards";
    const SCOPE: Scope = Scope::Project;
    const PATH_TEMPLATE: &'static str = "projects/{project}/dashboards/{name}.yaml";

    fn validate_spec(&self, meta: &ObjectMeta) -> Result<()> {
        names::validate_dns1123_label(&meta.name)?;
        self.validate()
    }
}

/// A `bar-chart` or `histogram` names the Endpoint it reads, the PascalCase type it counts and
/// the property it counts by, and no grid configuration (UI-93).
fn chart_widget(widget: &Widget) -> Result<()> {
    if widget.endpoint_ref.is_none() {
        return Err(Error::Name {
            field: "spec.pages[].widgets[].endpointRef",
            value: widget.widget_type.clone(),
            reason: "a chart widget names the Endpoint it reads through",
        });
    }
    match &widget.entity_type {
        Some(entity_type) => {
            names::validate_entity_type(entity_type).map_err(|_| Error::Name {
                field: "spec.pages[].widgets[].entityType",
                value: entity_type.clone(),
                reason: "an entity type is PascalCase, as NGSI-LD writes it",
            })?;
        }
        None => {
            return Err(Error::Name {
                field: "spec.pages[].widgets[].entityType",
                value: widget.widget_type.clone(),
                reason: "a chart widget names the entity type it counts",
            });
        }
    }
    if widget
        .property
        .as_deref()
        .is_none_or(|p| p.trim().is_empty())
    {
        return Err(Error::Name {
            field: "spec.pages[].widgets[].property",
            value: widget.widget_type.clone(),
            reason: "a chart widget names the property it counts by",
        });
    }
    if widget.grid.is_some() {
        return Err(Error::Name {
            field: "spec.pages[].widgets[].grid",
            value: widget.widget_type.clone(),
            reason: "`entityType` and `grid` belong to a widget of type `grid`",
        });
    }
    if widget
        .q
        .as_deref()
        .is_some_and(|q| q.chars().count() > MAX_CHART_Q || q.chars().any(char::is_control))
    {
        return Err(Error::Name {
            field: "spec.pages[].widgets[].q",
            value: widget.widget_type.clone(),
            reason: "a chart's `q` is one line of at most 1024 characters",
        });
    }
    Ok(())
}

/// A `vega-lite` widget (T-6786): its Endpoint, one of that Endpoint's sources, and a Vega-Lite 5
/// spec that passes the refusal list of ADR-N-053 §2.3, the pinned schema and the expression
/// allow-list. `at` is the widget's JSON pointer, so a refusal names the member it is about.
fn vega_lite_widget(widget: &Widget, at: &str) -> Result<()> {
    let invalid = |field: String, reason: String| Error::Invalid { field, reason };
    if widget.endpoint_ref.is_none() {
        return Err(invalid(
            format!("{at}/endpointRef"),
            "a vega-lite widget names the Endpoint it reads through".into(),
        ));
    }
    for (member, present) in [
        ("entityType", widget.entity_type.is_some()),
        ("entityId", widget.entity_id.is_some()),
        ("property", widget.property.is_some()),
        ("q", widget.q.is_some()),
        ("grid", widget.grid.is_some()),
    ] {
        if present {
            return Err(invalid(
                format!("{at}/{member}"),
                format!("a vega-lite widget names its rows in `data`, not in `{member}`"),
            ));
        }
    }
    let Some(data) = &widget.data else {
        return Err(invalid(
            format!("{at}/data"),
            "a vega-lite widget names the source of its Endpoint it draws, e.g. `source: csv`"
                .into(),
        ));
    };
    visualization::check_source(&format!("{at}/data"), data)?;
    let Some(spec) = &widget.spec else {
        return Err(invalid(
            format!("{at}/spec"),
            "a vega-lite widget carries its Vega-Lite 5 spec".into(),
        ));
    };
    let is_vega_lite = spec
        .get("$schema")
        .and_then(Value::as_str)
        .is_some_and(|s| s.starts_with("https://vega.github.io/schema/vega-lite/v5"));
    if !is_vega_lite {
        return Err(invalid(
            format!("{at}/spec/$schema"),
            "a vega-lite widget names https://vega.github.io/schema/vega-lite/v5.json".into(),
        ));
    }
    visualization::check_spec(spec).map_err(|r| {
        invalid(
            format!("{at}/spec{}", r.pointer),
            format!("{}: {}", r.rule, r.reason),
        )
    })?;
    let mut params = Vec::new();
    param_names(spec, &mut params);
    expressions(spec, &format!("{at}/spec"), &params)
}

/// The names the spec's `params` declare: a selection or a variable an expression may read.
fn param_names(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Array(params)) = map.get("params") {
                out.extend(
                    params
                        .iter()
                        .filter_map(|p| p.get("name").and_then(Value::as_str))
                        .map(str::to_owned),
                );
            }
            map.values().for_each(|v| param_names(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| param_names(v, out)),
        _ => {}
    }
}

/// Every member that holds an expression: `expr`, `calculate`, `signal`, `update`, any `…Expr`,
/// and `filter` or `test` given as a string. Inline rows are data and are not read.
fn expressions(value: &Value, at: &str, params: &[String]) -> Result<()> {
    match value {
        Value::Object(map) => {
            for (key, member) in map {
                let here = format!("{at}/{}", key.replace('~', "~0").replace('/', "~1"));
                if key == "values" {
                    continue;
                }
                let holds_expression =
                    matches!(key.as_str(), "expr" | "calculate" | "signal" | "update")
                        || key.ends_with("Expr")
                        || matches!(key.as_str(), "filter" | "test");
                match member {
                    Value::String(text) if holds_expression => expression(text, &here, params)?,
                    other => expressions(other, &here, params)?,
                }
            }
            Ok(())
        }
        Value::Array(items) => items
            .iter()
            .enumerate()
            .try_for_each(|(i, v)| expressions(v, &format!("{at}/{i}"), params)),
        _ => Ok(()),
    }
}

/// One expression against the allow-list: literals, operators, `datum` and its fields, the
/// spec's own params and [`EXPRESSION_FUNCTIONS`]. Any other name (`window`, `event`, a signal of
/// the view) and `__proto__`, `constructor` or `prototype` anywhere are refused.
pub fn expression(text: &str, at: &str, params: &[String]) -> Result<()> {
    let refuse = |reason: String| {
        Err(Error::Invalid {
            field: at.to_owned(),
            reason,
        })
    };
    if text.chars().count() > MAX_EXPRESSION {
        return refuse(format!(
            "an expression is at most {MAX_EXPRESSION} characters"
        ));
    }
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut after_dot = false;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != c {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            if j >= chars.len() {
                return refuse("an expression's text is not closed".into());
            }
            let literal: String = chars[i + 1..j].iter().collect();
            if forbidden(&literal) {
                return refuse("no __proto__, constructor or prototype".into());
            }
            i = j + 1;
            after_dot = false;
            continue;
        }
        if c.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            after_dot = false;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            if forbidden(&name) {
                return refuse("no __proto__, constructor or prototype".into());
            }
            // A member after `.` is a field of what precedes it (`datum.pm10`), not a name.
            if !after_dot
                && !EXPRESSION_FUNCTIONS.contains(&name.as_str())
                && !params.contains(&name)
            {
                return refuse(format!(
                    "`{name}` is not on the expression allow-list: use datum fields, the spec's params and {}",
                    "functions such as round, toNumber, timeFormat"
                ));
            }
            after_dot = false;
            continue;
        }
        if "+-*/%<>=!&|?:,()[].".contains(c) {
            after_dot = c == '.';
            i += 1;
            continue;
        }
        return refuse(format!("`{c}` has no place in an expression"));
    }
    Ok(())
}

fn forbidden(text: &str) -> bool {
    ["__proto__", "constructor", "prototype"]
        .iter()
        .any(|w| text.contains(w))
}

impl DashboardSpec {
    /// Every page shows something, and every layer it names is a name a Layer can have.
    pub fn validate(&self) -> Result<()> {
        if self.pages.is_empty() {
            return Err(Error::Name {
                field: "spec.pages",
                value: String::new(),
                reason: "a dashboard has at least one page (UI-17)",
            });
        }
        for (p, page) in self.pages.iter().enumerate() {
            if page.layers.is_empty() && page.widgets.is_empty() {
                return Err(Error::Name {
                    field: "spec.pages[]",
                    value: page.title.clone().unwrap_or_default(),
                    reason: "a page names at least one layer or widget",
                });
            }
            for layer in &page.layers {
                names::validate_dns1123_label(layer).map_err(|_| Error::Name {
                    field: "spec.pages[].layers[]",
                    value: layer.clone(),
                    reason: "a layer is named by its manifest's `metadata.name`",
                })?;
            }
            for (w, widget) in page.widgets.iter().enumerate() {
                if widget.widget_type.trim().is_empty() {
                    return Err(Error::Name {
                        field: "spec.pages[].widgets[].widgetType",
                        value: String::new(),
                        reason: "a widget names its type, e.g. `temporal-chart`",
                    });
                }
                if let Some(endpoint) = &widget.endpoint_ref {
                    names::validate_dns1123_label(endpoint).map_err(|_| Error::Name {
                        field: "spec.pages[].widgets[].endpointRef",
                        value: endpoint.clone(),
                        reason: "an endpoint is named by its manifest's `metadata.name`",
                    })?;
                }
                // The entity grid reads one Endpoint's entities of one type, like a layer does:
                // without both it would open on nothing, and its configuration is checked by the
                // same rules the SDK's parser applies in the browser (UI-71, SDK-30, T-1440).
                if widget.widget_type == GRID_WIDGET {
                    if widget.endpoint_ref.is_none() {
                        return Err(Error::Name {
                            field: "spec.pages[].widgets[].endpointRef",
                            value: String::new(),
                            reason: "a grid widget names the Endpoint it reads through",
                        });
                    }
                    match &widget.entity_type {
                        Some(entity_type) => {
                            names::validate_entity_type(entity_type).map_err(|_| Error::Name {
                                field: "spec.pages[].widgets[].entityType",
                                value: entity_type.clone(),
                                reason: "an entity type is PascalCase, as NGSI-LD writes it",
                            })?;
                        }
                        None => {
                            return Err(Error::Name {
                                field: "spec.pages[].widgets[].entityType",
                                value: String::new(),
                                reason: "a grid widget names the entity type it shows",
                            });
                        }
                    }
                    if let Some(config) = &widget.grid {
                        config.validate("spec.pages[].widgets[].grid")?;
                    }
                } else if TYPE_CHARTS.contains(&widget.widget_type.as_str()) {
                    chart_widget(widget)?;
                } else if widget.widget_type == VEGA_LITE_WIDGET {
                    vega_lite_widget(widget, &format!("/spec/pages/{p}/widgets/{w}"))?;
                } else if widget.data.is_some() || widget.spec.is_some() {
                    return Err(Error::Name {
                        field: "spec.pages[].widgets[].widgetType",
                        value: widget.widget_type.clone(),
                        reason: "`data` and `spec` belong to a widget of type `vega-lite`",
                    });
                } else if widget.entity_type.is_some() || widget.grid.is_some() {
                    return Err(Error::Name {
                        field: "spec.pages[].widgets[].widgetType",
                        value: widget.widget_type.clone(),
                        reason: "`entityType` and `grid` belong to a widget of type `grid`",
                    });
                }
                if widget.q.is_some() && !TYPE_CHARTS.contains(&widget.widget_type.as_str()) {
                    return Err(Error::Name {
                        field: "spec.pages[].widgets[].q",
                        value: widget.widget_type.clone(),
                        reason: "`q` belongs to a `bar-chart` or a `histogram`",
                    });
                }
            }
        }
        Ok(())
    }
}

/// How a layer draws (UI-20, UI-21): the three MapLibre styles and the aggregations only
/// deck.gl renders; `icon` draws as `circle` until the Portal has an icon renderer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LayerStyle {
    /// A point per entity.
    #[default]
    Circle,
    /// A line per entity.
    Line,
    /// A filled polygon per entity.
    Fill,
    /// A density surface over the points.
    Heatmap,
    /// Hexagonal bins over the points.
    Hexagon,
    /// A symbol per entity.
    Icon,
}

/// The static NGSI-LD filter every request of the layer carries (UI-18, UI-22).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LayerFilter {
    /// The `q` expression, e.g. `pm10>0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// The `scopeQ` expression, e.g. `/geo/FI/HKI/#`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_q: Option<String>,
    /// A fixed `geoQ`; the map adds its own viewport bbox on top (UI-22).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_q: Option<String>,
}

/// Colour by one numeric property over a domain (UI-18).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ColorBy {
    /// The property read from every feature.
    pub property: String,
    /// A ColorBrewer ramp name, e.g. `YlOrRd`; the Portal's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<String>,
    /// `[min, max]` of the property mapped onto the ramp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<[f64; 2]>,
}

/// Size by one numeric property over a pixel range (UI-18).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SizeBy {
    /// The property read from every feature.
    pub property: String,
    /// `[min, max]` radius in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
}

fn is_true(value: &bool) -> bool {
    *value
}

fn yes() -> bool {
    true
}

/// `spec` of a Layer (UI-18).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LayerSpec {
    /// The Endpoint of the same project the layer reads through, by `metadata.name`.
    pub source_endpoint_ref: String,
    /// The NGSI-LD entity type it shows.
    pub entity_type: String,
    /// How it draws.
    #[serde(default)]
    pub style: LayerStyle,
    /// Drawn when the dashboard opens; `false` leaves it in the legend, switched off.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    /// The static filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<LayerFilter>,
    /// The colour encoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_by: Option<ColorBy>,
    /// The size encoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_by: Option<SizeBy>,
    /// The properties a click shows, in order; the first few of the feature when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub popup_properties: Vec<String>,
}

impl Kind for LayerSpec {
    const KIND: &'static str = "Layer";
    const PLURAL: &'static str = "layers";
    const SCOPE: Scope = Scope::Project;
    const PATH_TEMPLATE: &'static str = "projects/{project}/dashboards/{name}.yaml";

    fn validate_spec(&self, meta: &ObjectMeta) -> Result<()> {
        names::validate_dns1123_label(&meta.name)?;
        self.validate()
    }
}

fn ordered(field: &'static str, pair: &[f64; 2], at_least: f64) -> Result<()> {
    let [min, max] = *pair;
    if !(min.is_finite() && max.is_finite()) || min < at_least || min >= max {
        return Err(Error::Name {
            field,
            value: format!("[{min}, {max}]"),
            reason: "`[min, max]` with min below max, both finite and not negative",
        });
    }
    Ok(())
}

impl LayerSpec {
    /// The endpoint is a manifest name, the type is an entity type, the encodings' ranges are
    /// ordered, and every popup property and filter expression is non-empty.
    pub fn validate(&self) -> Result<()> {
        names::validate_dns1123_label(&self.source_endpoint_ref).map_err(|_| Error::Name {
            field: "spec.sourceEndpointRef",
            value: self.source_endpoint_ref.clone(),
            reason: "an endpoint is named by its manifest's `metadata.name`",
        })?;
        names::validate_entity_type(&self.entity_type).map_err(|_| Error::Name {
            field: "spec.entityType",
            value: self.entity_type.clone(),
            reason: "an entity type is PascalCase, e.g. `AirQualityObserved` (PF-42)",
        })?;
        if let Some(filter) = &self.filter {
            for (field, value) in [
                ("spec.filter.q", &filter.q),
                ("spec.filter.scopeQ", &filter.scope_q),
                ("spec.filter.geoQ", &filter.geo_q),
            ] {
                if value.as_deref().is_some_and(|v| v.trim().is_empty()) {
                    return Err(Error::Name {
                        field,
                        value: String::new(),
                        reason: "a filter expression is not empty; leave the member out",
                    });
                }
            }
        }
        if let Some(color) = &self.color_by {
            if color.property.trim().is_empty() {
                return Err(Error::Name {
                    field: "spec.colorBy.property",
                    value: String::new(),
                    reason: "the property the colour reads is named",
                });
            }
            if let Some(domain) = &color.domain {
                ordered("spec.colorBy.domain", domain, f64::NEG_INFINITY)?;
            }
        }
        if let Some(size) = &self.size_by {
            if size.property.trim().is_empty() {
                return Err(Error::Name {
                    field: "spec.sizeBy.property",
                    value: String::new(),
                    reason: "the property the size reads is named",
                });
            }
            if let Some(range) = &size.range {
                ordered("spec.sizeBy.range", range, 0.0)?;
            }
        }
        if self.popup_properties.iter().any(|p| p.trim().is_empty()) {
            return Err(Error::Name {
                field: "spec.popupProperties[]",
                value: String::new(),
                reason: "a popup property is a property name",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! T-6786: the `vega-lite` widget. Unit tests, since CI runs jc-core's lib tests and not
    //! its `it` binary (which compares files the jc repository does not vendor).
    use super::*;
    use crate::kinds::Dashboard;

    const DASHBOARD: &str =
        include_str!("../../tests/golden/025-Dashboard-10-dashboards-and-visualization.yaml");

    /// The golden dashboard with one `vega-lite` widget appended to its analysis page, as JSON: the
    /// light format an author writes (T-6786).
    fn vega_lite(widget: serde_json::Value) -> Result<Dashboard> {
        let mut value: serde_json::Value =
            serde_norway::from_str(DASHBOARD).expect("golden is YAML");
        let widgets = value["spec"]["pages"][1]["widgets"]
            .as_array_mut()
            .expect("the analysis page has widgets");
        widgets.push(widget);
        let manifest = Dashboard::from_yaml(&serde_json::to_string(&value).expect("JSON"))
            .map_err(|e| Error::Parse(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn field(result: Result<Dashboard>) -> (String, String) {
        match result {
            Err(Error::Invalid { field, reason }) => (field, reason),
            Err(Error::Name { field, reason, .. }) => (field.to_owned(), reason.to_owned()),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn bars(spec_extra: serde_json::Value) -> serde_json::Value {
        let mut spec = serde_json::json!({
            "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
            "data": {"name": "source"},
            "params": [{"name": "pick", "select": {"type": "point", "fields": ["district"]}}],
            "mark": "bar",
            "encoding": {
                "x": {"field": "district", "type": "nominal"},
                "y": {"aggregate": "count", "type": "quantitative"},
                "opacity": {"condition": {"param": "pick", "value": 1}, "value": 0.4}
            },
            "transform": [{"filter": "isValid(datum.pm10) && datum.pm10 > 0"}]
        });
        if let (Some(base), Some(extra)) = (spec.as_object_mut(), spec_extra.as_object()) {
            base.extend(extra.clone());
        }
        serde_json::json!({
            "widgetType": "vega-lite",
            "endpointRef": "ep-air-quality",
            "data": {"source": "csv", "type": "AirQualityObserved"},
            "spec": spec
        })
    }

    /// T-6786: a `vega-lite` widget is a Vega-Lite 5 spec over one source of its Endpoint, with a
    /// selection and expressions from the allow-list, and it round trips.
    #[test]
    fn a_vega_lite_widget_draws_one_source_of_its_endpoint() {
        let dashboard = vega_lite(bars(serde_json::json!({}))).expect("validates");
        let widget = &dashboard.spec.pages[1].widgets[4];
        assert_eq!(widget.widget_type, "vega-lite");
        let yaml = serde_norway::to_string(&dashboard).expect("serializes");
        assert_eq!(Dashboard::from_yaml(&yaml).expect("parses back"), dashboard);
    }

    /// T-6786: a spec is data, never code: a URL, a name off the allow-list, Vega itself, a widget
    /// without its source and `spec` on another widget are refused, each naming its JSON path.
    #[test]
    fn a_vega_lite_widget_refuses_urls_code_and_a_missing_source_naming_the_member() {
        let at = "/spec/pages/1/widgets/4";
        let cases = [
            (
                bars(serde_json::json!({"data": {"url": "https://evil.example/rows.csv"}})),
                format!("{at}/spec/data/url"),
                "never a URL",
            ),
            (
                bars(
                    serde_json::json!({"transform": [{"calculate": "window.location", "as": "x"}]}),
                ),
                format!("{at}/spec/transform/0/calculate"),
                "`window` is not on the expression allow-list",
            ),
            (
                bars(serde_json::json!({"transform": [{"filter": "datum['__proto__']"}]})),
                format!("{at}/spec/transform/0/filter"),
                "__proto__",
            ),
            (
                bars(serde_json::json!({"params": [{"name": "p", "expr": "event.x"}]})),
                format!("{at}/spec/params/0/expr"),
                "`event`",
            ),
            (
                bars(serde_json::json!({"$schema": "https://vega.github.io/schema/vega/v5.json"})),
                format!("{at}/spec/$schema"),
                "vega-lite/v5.json",
            ),
            (
                bars(serde_json::json!({"usermeta": {"x": 1}})),
                format!("{at}/spec/usermeta"),
                "usermeta",
            ),
        ];
        for (widget, path, words) in cases {
            let (field_at, reason) = field(vega_lite(widget));
            assert_eq!(field_at, path, "{reason}");
            assert!(reason.contains(words), "{path}: {reason}");
        }

        let mut no_data = bars(serde_json::json!({}));
        no_data.as_object_mut().map(|w| w.remove("data"));
        assert_eq!(field(vega_lite(no_data)).0, format!("{at}/data"));

        let mut no_endpoint = bars(serde_json::json!({}));
        no_endpoint.as_object_mut().map(|w| w.remove("endpointRef"));
        assert_eq!(field(vega_lite(no_endpoint)).0, format!("{at}/endpointRef"));

        let mut long = bars(serde_json::json!({}));
        long["data"] = serde_json::json!({"source": "temporal", "type": "AirQualityObserved", "attrs": ["pm10"], "last": "P90D"});
        assert_eq!(field(vega_lite(long)).0, format!("{at}/data/last"));

        let mut typed = bars(serde_json::json!({}));
        typed["entityType"] = serde_json::json!("AirQualityObserved");
        assert_eq!(field(vega_lite(typed)).0, format!("{at}/entityType"));

        let mut elsewhere = bars(serde_json::json!({}));
        elsewhere["widgetType"] = serde_json::json!("temporal-chart");
        assert!(field(vega_lite(elsewhere))
            .1
            .contains("belong to a widget of type `vega-lite`"));
    }

    /// T-6786: the expression allow-list takes fields, the spec's params and the listed functions.
    #[test]
    fn the_expression_allow_list_takes_fields_params_and_listed_functions() {
        let params = ["pick".to_owned()];
        for ok in [
            "datum.pm10 > 40 && isValid(datum['no2'])",
            "timeFormat(toDate(datum.observedAt), '%H:%M')",
            "pick && round(datum.x * 100) / 100",
            "upper(datum.name) + ' (' + datum.city + ')'",
        ] {
            assert!(expression(ok, "/e", &params).is_ok(), "{ok}");
        }
        for refused in [
            "alert(1)",
            "datum.constructor",
            "width",
            "'a' ; 1",
            "\"unclosed",
            "`template`",
        ] {
            assert!(expression(refused, "/e", &params).is_err(), "{refused}");
        }
    }
}
