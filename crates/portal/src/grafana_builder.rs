//! The dashboard builder's one model (owner 2026-11-07 and 2026-11-08): a Grafana dashboard JSON
//! file in the project's repository, `projects/{project}/grafana/{uid}.json`. A person builds it
//! in the Portal from choices only ([`Board`]); the Portal compiles the choices into Grafana's
//! own panels and NGSI-LD plugin targets ([`compile`]), proposes the file as a Change, and once
//! it is approved [`DashboardSync`] provisions it into the project's organization of the
//! dashboards service. [`decompile`] reads a file back into choices for the next edit.
//!
//! The browser never sends a query: a [`Board`] has no member for one, and `deny_unknown_fields`
//! refuses every member it does not name.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::config::GrafanaConfig;
use crate::grafana::Client;

/// The datasource type of the NGSI-LD plugin, as `crate::grafana` names it.
const PLUGIN: &str = "ngsild-grafana-datasource";

/// Every builder dashboard's uid starts so; nothing else in an organization does (`pv-` is the
/// visualization page's, provisioned dashboards carry their own), so the sync removes only its own.
pub const UID_PREFIX: &str = "jcd-";

/// The directory of a project holding the dashboard files.
const DIR: &str = "grafana";

pub const MAX_TITLE: usize = 120;
pub const MAX_DESCRIPTION: usize = 500;
pub const MAX_PANELS: usize = 24;
pub const MAX_VARIABLES: usize = 8;
pub const MAX_FILTERS: usize = 8;
pub const MAX_THRESHOLDS: usize = 8;
/// The longest filter value a person types.
pub const MAX_VALUE: usize = 128;
/// The most values a variable offers.
pub const MAX_OPTIONS: usize = 200;
/// The largest dashboard file the door writes and the sync provisions.
pub const MAX_FILE: usize = 256 * 1024;

/// The refresh intervals the builder offers; empty is off.
pub const REFRESHES: [&str; 8] = ["", "1m", "5m", "15m", "30m", "1h", "2h", "1d"];

/// The units the builder offers, by Grafana's unit id; empty is none.
pub const UNITS: [&str; 24] = [
    "",
    "short",
    "percent",
    "percentunit",
    "celsius",
    "fahrenheit",
    "humidity",
    "pressurehpa",
    "lengthm",
    "lengthkm",
    "velocityms",
    "velocitykmh",
    "conμgm3",
    "conmgm3",
    "ppm",
    "dB",
    "watt",
    "kwatt",
    "kwatth",
    "litre",
    "m3",
    "currencyEUR",
    "s",
    "none",
];

/// The colours a threshold step takes, Grafana's named ones.
pub const COLORS: [&str; 7] = ["green", "yellow", "orange", "red", "blue", "purple", "text"];

/// What a panel draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PanelKind {
    Timeseries,
    Stat,
    Table,
    Gauge,
}

impl PanelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PanelKind::Timeseries => "timeseries",
            PanelKind::Stat => "stat",
            PanelKind::Table => "table",
            PanelKind::Gauge => "gauge",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Self::Timeseries, Self::Stat, Self::Table, Self::Gauge]
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }

    /// The visualization page's kind with the same needs: a gauge draws what a stat draws.
    pub fn drawn_as(self) -> crate::grafana::Kind {
        match self {
            PanelKind::Timeseries => crate::grafana::Kind::Timeseries,
            PanelKind::Stat | PanelKind::Gauge => crate::grafana::Kind::Stat,
            PanelKind::Table => crate::grafana::Kind::Table,
        }
    }
}

/// How readings are combined: by the broker per interval for a time series, over the range for a
/// stat or gauge. `none` draws each reading (a time series) or the latest value (a stat, a gauge).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Aggregation {
    #[default]
    None,
    Avg,
    Min,
    Max,
    Sum,
}

impl Aggregation {
    const ALL: [Aggregation; 5] = [Self::None, Self::Avg, Self::Min, Self::Max, Self::Sum];

    fn method(self) -> Option<&'static str> {
        match self {
            Aggregation::None => None,
            Aggregation::Avg => Some("avg"),
            Aggregation::Min => Some("min"),
            Aggregation::Max => Some("max"),
            Aggregation::Sum => Some("sum"),
        }
    }

    /// Grafana's reducer for a stat or gauge.
    fn calc(self) -> &'static str {
        match self {
            Aggregation::None => "lastNotNull",
            Aggregation::Avg => "mean",
            Aggregation::Min => "min",
            Aggregation::Max => "max",
            Aggregation::Sum => "sum",
        }
    }
}

/// The period the broker aggregates over; `auto` follows the panel's width and range.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum Interval {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "PT1M")]
    Minute,
    #[serde(rename = "PT5M")]
    FiveMinutes,
    #[serde(rename = "PT15M")]
    QuarterHour,
    #[serde(rename = "PT1H")]
    Hour,
    #[serde(rename = "P1D")]
    Day,
}

impl Interval {
    const ALL: [Interval; 6] = [
        Self::Auto,
        Self::Minute,
        Self::FiveMinutes,
        Self::QuarterHour,
        Self::Hour,
        Self::Day,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Interval::Auto => "auto",
            Interval::Minute => "PT1M",
            Interval::FiveMinutes => "PT5M",
            Interval::QuarterHour => "PT15M",
            Interval::Hour => "PT1H",
            Interval::Day => "P1D",
        }
    }
}

/// A filter's comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Op {
    fn word(self) -> &'static str {
        match self {
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Lt => "<",
            Op::Le => "<=",
        }
    }
}

/// Where a panel sits on Grafana's 24-column grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Grid {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// One filter: an attribute compared with a value, or matched against a variable's picks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Filter {
    pub attribute: String,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// A variable of the dashboard; then `op` is `eq` and there is no `value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
}

/// One threshold step above the base colour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ThresholdStep {
    pub value: f64,
    pub color: String,
}

/// A panel's colours by value: the base colour, then each step from its value up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    pub base: String,
    #[serde(default)]
    pub steps: Vec<ThresholdStep>,
}

/// One panel, as choices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BoardPanel {
    pub id: u32,
    pub kind: PanelKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub grid: Grid,
    /// An Endpoint of the project, by name.
    pub endpoint: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub attribute: String,
    #[serde(default)]
    pub aggregation: Aggregation,
    #[serde(default)]
    pub interval: Interval,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<Thresholds>,
}

/// A variable over the current values of one attribute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BoardVariable {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub endpoint: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub attribute: String,
    #[serde(default)]
    pub include_all: bool,
}

/// A dashboard, as choices: what the builder sends and reads back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Board {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The default range, Grafana's words (`now-6h`, `now`).
    #[serde(default = "default_from")]
    pub from: String,
    #[serde(default = "default_to")]
    pub to: String,
    /// One of [`REFRESHES`]; empty is off.
    #[serde(default)]
    pub refresh: String,
    #[serde(default)]
    pub variables: Vec<BoardVariable>,
    #[serde(default)]
    pub panels: Vec<BoardPanel>,
}

fn default_from() -> String {
    "now-6h".into()
}

fn default_to() -> String {
    "now".into()
}

/// A refusal of one field of a [`Board`]: its path and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub field: String,
    pub detail: String,
}

fn refuse(field: impl Into<String>, detail: impl Into<String>) -> Refusal {
    Refusal {
        field: field.into(),
        detail: detail.into(),
    }
}

/// A person's text: trimmed, within `max` characters, no control character, nothing reading
/// like a credential.
fn text_ok(text: &str, max: usize, required: bool) -> Result<(), String> {
    let trimmed = text.trim();
    if required && trimmed.is_empty() {
        return Err(format!("give 1 to {max} characters"));
    }
    if text.chars().count() > max {
        return Err(format!("{max} characters at most"));
    }
    if text.chars().any(char::is_control) {
        return Err("no line breaks or control characters".into());
    }
    if crate::secret_scan::secret_text(text) {
        return Err("this reads like a credential; write something else".into());
    }
    Ok(())
}

/// A name the model and the NGSI-LD `q` both take as they are.
fn plain_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A variable name Grafana takes in `${name}` and the builder reserves nothing of.
pub fn variable_name_ok(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.as_bytes()[0].is_ascii_alphabetic()
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A value that stays one literal inside a `q` text: no quote, separator, escape or grouping.
pub fn value_ok(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_VALUE
        && !value
            .chars()
            .any(|c| c.is_control() || "\"\\;|(),".contains(c))
}

/// Everything about a board that needs no model: lengths, caps, the grid, names, units, colours
/// and how the choices fit together. The first refusal, by field.
pub fn check_shape(board: &Board) -> Result<(), Refusal> {
    text_ok(&board.title, MAX_TITLE, true).map_err(|d| refuse("title", d))?;
    text_ok(&board.description, MAX_DESCRIPTION, false).map_err(|d| refuse("description", d))?;
    for (field, word) in [("from", &board.from), ("to", &board.to)] {
        if !crate::api::grafana_dashboards::is_time(word) {
            return Err(refuse(
                field,
                format!("'{word}' is not a time such as now-6h or now"),
            ));
        }
    }
    if !REFRESHES.contains(&board.refresh.as_str()) {
        return Err(refuse(
            "refresh",
            format!("choose one of {}", REFRESHES[1..].join(", ")),
        ));
    }
    if board.variables.len() > MAX_VARIABLES {
        return Err(refuse(
            "variables",
            format!("{MAX_VARIABLES} variables at most"),
        ));
    }
    let mut names = BTreeSet::new();
    for (i, variable) in board.variables.iter().enumerate() {
        let at = format!("variables[{i}]");
        if !variable_name_ok(&variable.name) {
            return Err(refuse(
                format!("{at}.name"),
                "a letter, then letters, digits or _, 32 at most, for example station",
            ));
        }
        if !names.insert(variable.name.as_str()) {
            return Err(refuse(
                format!("{at}.name"),
                format!("{} is named twice", variable.name),
            ));
        }
        text_ok(&variable.label, MAX_TITLE, false).map_err(|d| refuse(format!("{at}.label"), d))?;
        for (field, name) in [
            ("endpoint", &variable.endpoint),
            ("type", &variable.entity_type),
            ("attribute", &variable.attribute),
        ] {
            if !plain_name(name) {
                return Err(refuse(format!("{at}.{field}"), "choose one from the list"));
            }
        }
    }
    if board.panels.len() > MAX_PANELS {
        return Err(refuse("panels", format!("{MAX_PANELS} panels at most")));
    }
    let mut ids = BTreeSet::new();
    for (i, panel) in board.panels.iter().enumerate() {
        check_panel(&format!("panels[{i}]"), panel, &names)?;
        if !ids.insert(panel.id) {
            return Err(refuse(
                format!("panels[{i}].id"),
                format!("panel {} is there twice", panel.id),
            ));
        }
        for (j, other) in board.panels[..i].iter().enumerate() {
            if overlaps(panel.grid, other.grid) {
                return Err(refuse(
                    format!("panels[{i}].grid"),
                    format!("it overlaps panel {} ({})", j + 1, other.title.trim()),
                ));
            }
        }
    }
    Ok(())
}

fn overlaps(a: Grid, b: Grid) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

fn check_panel(at: &str, panel: &BoardPanel, variables: &BTreeSet<&str>) -> Result<(), Refusal> {
    if !(1..=1000).contains(&panel.id) {
        return Err(refuse(format!("{at}.id"), "a number from 1 to 1000"));
    }
    text_ok(&panel.title, MAX_TITLE, true).map_err(|d| refuse(format!("{at}.title"), d))?;
    text_ok(&panel.description, MAX_DESCRIPTION, false)
        .map_err(|d| refuse(format!("{at}.description"), d))?;
    let grid = panel.grid;
    if grid.w == 0 || grid.x + grid.w > 24 || !(2..=40).contains(&grid.h) || grid.y > 500 {
        return Err(refuse(
            format!("{at}.grid"),
            "a place on the 24 columns: x + w at most 24, a height of 2 to 40 rows",
        ));
    }
    for (field, name) in [
        ("endpoint", &panel.endpoint),
        ("type", &panel.entity_type),
        ("attribute", &panel.attribute),
    ] {
        if !plain_name(name) {
            return Err(refuse(format!("{at}.{field}"), "choose one from the list"));
        }
    }
    match panel.kind {
        PanelKind::Table if panel.aggregation != Aggregation::None => {
            return Err(refuse(
                format!("{at}.aggregation"),
                "a table shows the current values; choose no aggregation",
            ))
        }
        _ => {}
    }
    let aggregates_in_time = panel.aggregation != Aggregation::None;
    if !aggregates_in_time && panel.interval != Interval::Auto {
        return Err(refuse(
            format!("{at}.interval"),
            "an interval needs an aggregation; choose one, or leave the interval on auto",
        ));
    }
    if !UNITS.contains(&panel.unit.as_str()) {
        return Err(refuse(format!("{at}.unit"), "choose a unit from the list"));
    }
    if panel.decimals.is_some_and(|d| d > 6) {
        return Err(refuse(format!("{at}.decimals"), "0 to 6 decimals"));
    }
    if let Some(thresholds) = &panel.thresholds {
        if !COLORS.contains(&thresholds.base.as_str()) {
            return Err(refuse(
                format!("{at}.thresholds.base"),
                "choose a colour from the list",
            ));
        }
        if thresholds.steps.len() > MAX_THRESHOLDS {
            return Err(refuse(
                format!("{at}.thresholds"),
                format!("{MAX_THRESHOLDS} steps at most"),
            ));
        }
        let mut last = f64::NEG_INFINITY;
        for (k, step) in thresholds.steps.iter().enumerate() {
            if !step.value.is_finite() || step.value <= last {
                return Err(refuse(
                    format!("{at}.thresholds.steps[{k}].value"),
                    "each step a number above the one before it",
                ));
            }
            if !COLORS.contains(&step.color.as_str()) {
                return Err(refuse(
                    format!("{at}.thresholds.steps[{k}].color"),
                    "choose a colour from the list",
                ));
            }
            last = step.value;
        }
    }
    if panel.filters.len() > MAX_FILTERS {
        return Err(refuse(
            format!("{at}.filters"),
            format!("{MAX_FILTERS} filters at most"),
        ));
    }
    for (k, filter) in panel.filters.iter().enumerate() {
        let here = format!("{at}.filters[{k}]");
        if !plain_name(&filter.attribute) {
            return Err(refuse(
                format!("{here}.attribute"),
                "choose one from the list",
            ));
        }
        match (&filter.value, &filter.variable) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(refuse(
                    here,
                    "compare with a value or with a variable, one of the two",
                ))
            }
            (None, Some(variable)) => {
                if !variables.contains(variable.as_str()) {
                    return Err(refuse(
                        format!("{here}.variable"),
                        format!("the dashboard has no variable {variable}"),
                    ));
                }
                if filter.op != Op::Eq {
                    return Err(refuse(
                        format!("{here}.op"),
                        "a variable matches by is; choose is",
                    ));
                }
            }
            (Some(value), None) => {
                if !value_ok(value) {
                    return Err(refuse(
                        format!("{here}.value"),
                        format!(
                            "1 to {MAX_VALUE} characters without quotes, brackets, commas, | or ;"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// How an attribute's values compare in `q`: as numbers, as true or false, or as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Number,
    Boolean,
    Text,
}

impl ValueKind {
    pub fn of(value_type: Option<&str>) -> Self {
        match value_type {
            Some("number" | "integer") => ValueKind::Number,
            Some("boolean") => ValueKind::Boolean,
            _ => ValueKind::Text,
        }
    }
}

/// The filter as a value check against its attribute's kind, by field.
pub fn check_filter(at: &str, filter: &Filter, kind: ValueKind) -> Result<(), Refusal> {
    if filter.variable.is_some() {
        return if kind == ValueKind::Text {
            Ok(())
        } else {
            Err(refuse(
                format!("{at}.attribute"),
                format!("{} is not text; a variable matches text", filter.attribute),
            ))
        };
    }
    let value = filter.value.as_deref().unwrap_or_default();
    match kind {
        ValueKind::Number if value.parse::<f64>().is_ok_and(f64::is_finite) => Ok(()),
        ValueKind::Number => Err(refuse(
            format!("{at}.value"),
            format!(
                "{} holds numbers; write one, for example 25",
                filter.attribute
            ),
        )),
        ValueKind::Boolean if matches!(filter.op, Op::Eq | Op::Ne) => {
            if value == "true" || value == "false" {
                Ok(())
            } else {
                Err(refuse(format!("{at}.value"), "true or false"))
            }
        }
        ValueKind::Text if matches!(filter.op, Op::Eq | Op::Ne) => Ok(()),
        _ => Err(refuse(
            format!("{at}.op"),
            format!("{} is not a number; choose is or is not", filter.attribute),
        )),
    }
}

/// One clause of the panel's `q`.
fn clause(filter: &Filter, kind: ValueKind) -> String {
    if let Some(variable) = &filter.variable {
        // Grafana writes a multi-value pick as an escaped `(a|b)`, All as the variable's `.*`.
        return format!("{}~=\"^${{{variable}:regex}}$\"", filter.attribute);
    }
    let value = filter.value.as_deref().unwrap_or_default();
    let literal = match kind {
        ValueKind::Text => format!("\"{value}\""),
        ValueKind::Number | ValueKind::Boolean => value.to_owned(),
    };
    format!("{}{}{literal}", filter.attribute, filter.op.word())
}

/// The filter a clause of the builder's own `q` stands for; `None` for any other text.
fn filter_of(clause: &str) -> Option<Filter> {
    if let Some((attribute, rest)) = clause.split_once("~=\"^${") {
        let variable = rest.strip_suffix(":regex}$\"")?;
        return (plain_name(attribute) && variable_name_ok(variable)).then(|| Filter {
            attribute: attribute.to_owned(),
            op: Op::Eq,
            value: None,
            variable: Some(variable.to_owned()),
        });
    }
    // The two-sign comparisons first, so `>=` is not read as `>` and `=…`.
    let (at, op) = [Op::Eq, Op::Ne, Op::Ge, Op::Le, Op::Gt, Op::Lt]
        .into_iter()
        .find_map(|op| clause.find(op.word()).map(|at| (at, op)))?;
    let attribute = &clause[..at];
    let raw = &clause[at + op.word().len()..];
    let value = raw
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(raw);
    (plain_name(attribute) && value_ok(value)).then(|| Filter {
        attribute: attribute.to_owned(),
        op,
        value: Some(value.to_owned()),
        variable: None,
    })
}

/// What [`compile`] needs of each panel's source beyond the choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The datasource uid the organization reads the panel's Endpoint through.
    pub datasource: String,
    /// The class declares a `name` to label each entity's series with.
    pub named: bool,
    /// How each filter's attribute compares, in the filters' order.
    pub filters: Vec<ValueKind>,
}

/// The variable's description: what it reads, which [`decompile`] reads back.
fn variable_description(variable: &BoardVariable) -> String {
    format!(
        "Values of {} ({}, endpoint {})",
        variable.attribute, variable.entity_type, variable.endpoint
    )
}

fn variable_source(description: &str) -> Option<(String, String, String)> {
    let rest = description.strip_prefix("Values of ")?;
    let (attribute, rest) = rest.split_once(" (")?;
    let (entity_type, rest) = rest.split_once(", endpoint ")?;
    let endpoint = rest.strip_suffix(')')?;
    [attribute, entity_type, endpoint]
        .iter()
        .all(|name| plain_name(name))
        .then(|| (attribute.into(), entity_type.into(), endpoint.into()))
}

/// The values a variable may offer: each a literal of a `q` and an item of Grafana's comma list.
pub fn offered(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut kept: Vec<String> = values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| value_ok(value) && !value.contains(" : "))
        .collect();
    kept.sort();
    kept.dedup();
    kept.truncate(MAX_OPTIONS);
    kept
}

fn compiled_variable(variable: &BoardVariable, values: &[String]) -> Value {
    let label = if variable.label.trim().is_empty() {
        variable.name.clone()
    } else {
        variable.label.trim().to_owned()
    };
    let current = if variable.include_all {
        json!({ "text": ["All"], "value": ["$__all"] })
    } else {
        let first: Vec<&String> = values.iter().take(1).collect();
        json!({ "text": first, "value": first })
    };
    let mut options: Vec<Value> = values
        .iter()
        .map(|value| json!({ "text": value, "value": value, "selected": false }))
        .collect();
    if variable.include_all {
        options.insert(
            0,
            json!({ "text": "All", "value": "$__all", "selected": true }),
        );
    }
    let mut out = json!({
        "name": variable.name,
        "label": label,
        "type": "custom",
        "description": variable_description(variable),
        "query": values.join(","),
        "multi": true,
        "includeAll": variable.include_all,
        "current": current,
        "options": options,
        "hide": 0,
    });
    if variable.include_all {
        out["allValue"] = json!(".*");
    }
    out
}

fn compiled_panel(panel: &BoardPanel, source: &Source) -> Value {
    let reference = json!({ "type": PLUGIN, "uid": source.datasource });
    let mut target = json!({
        "refId": "A",
        "datasource": reference,
        "entityType": panel.entity_type,
        "attributes": [panel.attribute],
        "limit": 100,
    });
    let temporal = panel.kind == PanelKind::Timeseries || panel.aggregation != Aggregation::None;
    if temporal {
        target["queryType"] = json!("temporal");
        if let Some(method) = panel.aggregation.method() {
            target["aggrMethod"] = json!(method);
            target["aggrPeriodDuration"] = json!(panel.interval.as_str());
        }
    } else {
        target["queryType"] = json!("entity");
        target["format"] = json!(if panel.kind == PanelKind::Table {
            "table"
        } else {
            "timeseries"
        });
    }
    if source.named {
        if panel.kind == PanelKind::Table {
            if panel.attribute != "name" {
                target["attributes"] = json!(["name", panel.attribute]);
            }
        } else {
            target["entityName"] = json!("name");
            target["namePattern"] = json!("entity_name");
        }
    }
    let q: Vec<String> = panel
        .filters
        .iter()
        .zip(
            source
                .filters
                .iter()
                .chain(std::iter::repeat(&ValueKind::Text)),
        )
        .map(|(filter, kind)| clause(filter, *kind))
        .collect();
    if !q.is_empty() {
        target["customQuery"] = json!(q.join(";"));
    }
    let mut defaults = serde_json::Map::new();
    if !panel.unit.is_empty() {
        defaults.insert("unit".into(), json!(panel.unit));
    }
    if let Some(decimals) = panel.decimals {
        defaults.insert("decimals".into(), json!(decimals));
    }
    if let Some(thresholds) = &panel.thresholds {
        let steps: Vec<Value> = std::iter::once(json!({ "color": thresholds.base, "value": null }))
            .chain(
                thresholds
                    .steps
                    .iter()
                    .map(|step| json!({ "color": step.color, "value": step.value })),
            )
            .collect();
        defaults.insert(
            "thresholds".into(),
            json!({ "mode": "absolute", "steps": steps }),
        );
        match panel.kind {
            PanelKind::Stat | PanelKind::Gauge => {
                defaults.insert("color".into(), json!({ "mode": "thresholds" }));
            }
            PanelKind::Timeseries => {
                defaults.insert(
                    "custom".into(),
                    json!({ "thresholdsStyle": { "mode": "line" } }),
                );
            }
            PanelKind::Table => {}
        }
    }
    let reduce = json!({ "calcs": [panel.aggregation.calc()], "fields": "", "values": false });
    let options = match panel.kind {
        PanelKind::Timeseries => json!({
            "legend": { "displayMode": "list", "placement": "bottom", "showLegend": true },
            "tooltip": { "mode": "single", "sort": "none" },
        }),
        PanelKind::Stat => json!({
            "reduceOptions": reduce,
            "textMode": "value_and_name",
            "colorMode": "value",
            "graphMode": "none",
        }),
        PanelKind::Gauge => json!({
            "reduceOptions": reduce,
            "showThresholdMarkers": true,
            "showThresholdLabels": false,
        }),
        PanelKind::Table => json!({ "showHeader": true }),
    };
    let mut out = json!({
        "id": panel.id,
        "type": panel.kind.as_str(),
        "title": panel.title.trim(),
        "gridPos": { "x": panel.grid.x, "y": panel.grid.y, "w": panel.grid.w, "h": panel.grid.h },
        "datasource": reference,
        "targets": [target],
        "fieldConfig": { "defaults": defaults, "overrides": [] },
        "options": options,
    });
    if !panel.description.trim().is_empty() {
        out["description"] = json!(panel.description.trim());
    }
    out
}

/// The Grafana dashboard JSON of `board`: `sources` and `values` follow `board.panels` and
/// `board.variables` in order. The folder is the project's (`project-{project}`), which the
/// provisioning call names, so the file names none.
pub fn compile(uid: &str, board: &Board, sources: &[Source], values: &[Vec<String>]) -> Value {
    let no_values = Vec::new();
    let variables: Vec<Value> = board
        .variables
        .iter()
        .enumerate()
        .map(|(i, v)| compiled_variable(v, values.get(i).unwrap_or(&no_values)))
        .collect();
    let panels: Vec<Value> = board
        .panels
        .iter()
        .zip(sources)
        .map(|(panel, source)| compiled_panel(panel, source))
        .collect();
    let mut out = json!({
        "uid": uid,
        "title": board.title.trim(),
        "tags": [],
        "editable": true,
        "schemaVersion": 39,
        "timezone": "browser",
        "time": { "from": board.from, "to": board.to },
        "refresh": board.refresh,
        "templating": { "list": variables },
        "panels": panels,
    });
    if !board.description.trim().is_empty() {
        out["description"] = json!(board.description.trim());
    }
    out
}

/// The dashboard file's members: Grafana's, and only those [`compile`] writes.
const FILE_KEYS: [&str; 11] = [
    "uid",
    "title",
    "description",
    "tags",
    "editable",
    "schemaVersion",
    "timezone",
    "time",
    "refresh",
    "templating",
    "panels",
];

/// A file the builder wrote, read back into choices. `endpoint_of` names the Endpoint a
/// datasource uid reads. `Err` says why the file is not the builder's to edit.
pub fn decompile(
    file: &Value,
    endpoint_of: &dyn Fn(&str) -> Option<String>,
) -> Result<Board, String> {
    let object = file.as_object().ok_or("the file holds no JSON object")?;
    if let Some(stray) = object.keys().find(|key| !FILE_KEYS.contains(&key.as_str())) {
        return Err(format!(
            "the file has a member the builder does not write: {stray}"
        ));
    }
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let mut variables = Vec::new();
    for variable in file["templating"]["list"].as_array().into_iter().flatten() {
        let name = text(&variable["name"]);
        let (attribute, entity_type, endpoint) =
            variable_source(variable["description"].as_str().unwrap_or_default())
                .ok_or_else(|| format!("variable {name} was not written by the builder"))?;
        let label = text(&variable["label"]);
        variables.push(BoardVariable {
            label: if label == name { String::new() } else { label },
            name,
            endpoint,
            entity_type,
            attribute,
            include_all: variable["includeAll"] == true,
        });
    }
    let mut panels = Vec::new();
    for panel in file["panels"].as_array().into_iter().flatten() {
        panels.push(decompiled_panel(panel, endpoint_of)?);
    }
    Ok(Board {
        title: text(&file["title"]),
        description: text(&file["description"]),
        from: file["time"]["from"].as_str().unwrap_or("now-6h").to_owned(),
        to: file["time"]["to"].as_str().unwrap_or("now").to_owned(),
        refresh: text(&file["refresh"]),
        variables,
        panels,
    })
}

fn decompiled_panel(
    panel: &Value,
    endpoint_of: &dyn Fn(&str) -> Option<String>,
) -> Result<BoardPanel, String> {
    let id = panel["id"]
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
        .ok_or("a panel without an id")?;
    let kind = PanelKind::parse(panel["type"].as_str().unwrap_or_default()).ok_or_else(|| {
        format!(
            "panel {id} is a {}, which the builder does not draw",
            panel["type"]
        )
    })?;
    let targets = panel["targets"].as_array().map_or(&[][..], Vec::as_slice);
    let [target] = targets else {
        return Err(format!(
            "panel {id} has {} queries; the builder writes one",
            targets.len()
        ));
    };
    let datasource = target["datasource"]["uid"].as_str().unwrap_or_default();
    let endpoint = endpoint_of(datasource).ok_or_else(|| {
        format!("panel {id} reads a datasource that reads no Endpoint of this project")
    })?;
    let attributes: Vec<&str> = target["attributes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|a| {
            !(kind == PanelKind::Table
                && *a == "name"
                && target["attributes"]
                    .as_array()
                    .is_some_and(|all| all.len() > 1))
        })
        .collect();
    let [attribute] = attributes[..] else {
        return Err(format!(
            "panel {id} reads {} attributes; the builder reads one",
            attributes.len()
        ));
    };
    let aggregation = match target["aggrMethod"].as_str() {
        None => Aggregation::None,
        Some(method) => Aggregation::ALL
            .into_iter()
            .find(|a| a.method() == Some(method))
            .ok_or_else(|| format!("panel {id} aggregates by {method}"))?,
    };
    let interval = match target["aggrPeriodDuration"].as_str() {
        None => Interval::Auto,
        Some(period) => Interval::ALL
            .into_iter()
            .find(|i| i.as_str() == period)
            .ok_or_else(|| format!("panel {id} aggregates over {period}"))?,
    };
    let filters = match target["customQuery"].as_str().filter(|q| !q.is_empty()) {
        None => Vec::new(),
        Some(q) => q
            .split(';')
            .map(|c| {
                filter_of(c)
                    .ok_or_else(|| format!("panel {id} filters by text the builder does not write"))
            })
            .collect::<Result<_, _>>()?,
    };
    let defaults = &panel["fieldConfig"]["defaults"];
    let thresholds = defaults["thresholds"]["steps"]
        .as_array()
        .and_then(|steps| {
            let (base, rest) = steps.split_first()?;
            Some(Thresholds {
                base: base["color"].as_str()?.to_owned(),
                steps: rest
                    .iter()
                    .filter_map(|s| {
                        Some(ThresholdStep {
                            value: s["value"].as_f64()?,
                            color: s["color"].as_str()?.to_owned(),
                        })
                    })
                    .collect(),
            })
        });
    let grid = &panel["gridPos"];
    let at = |key: &str| {
        grid[key]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    };
    Ok(BoardPanel {
        id,
        kind,
        title: panel["title"].as_str().unwrap_or_default().to_owned(),
        description: panel["description"].as_str().unwrap_or_default().to_owned(),
        grid: Grid {
            x: at("x"),
            y: at("y"),
            w: at("w"),
            h: at("h"),
        },
        endpoint,
        entity_type: target["entityType"].as_str().unwrap_or_default().to_owned(),
        attribute: attribute.to_owned(),
        aggregation,
        interval,
        filters,
        unit: defaults["unit"].as_str().unwrap_or_default().to_owned(),
        decimals: defaults["decimals"]
            .as_u64()
            .and_then(|d| u8::try_from(d).ok()),
        thresholds,
    })
}

/// The repository path of a project's dashboard.
pub fn path_of(project: &str, uid: &str) -> String {
    format!("projects/{project}/{DIR}/{uid}.json")
}

/// The project and uid of a dashboard file, the inverse of [`path_of`]; `None` for any other path.
pub fn parse_path(path: &str) -> Option<(&str, &str)> {
    let (project, file) = path
        .strip_prefix("projects/")?
        .split_once(&format!("/{DIR}/"))?;
    let uid = file.strip_suffix(".json")?;
    (crate::resource::is_dns1123(project) && is_builder_uid(uid)).then_some((project, uid))
}

/// Whether a repository path is a builder dashboard file a stage writes: [`parse_path`]'s shape
/// with any one project segment, as a project repository lists its tree under `projects/_/`.
pub fn is_dashboard_file(path: &str) -> bool {
    let Some((project, file)) = path
        .trim_start_matches('/')
        .strip_prefix("projects/")
        .and_then(|rest| rest.split_once(&format!("/{DIR}/")))
    else {
        return false;
    };
    !project.is_empty()
        && !project.contains('/')
        && project != ".."
        && file.strip_suffix(".json").is_some_and(is_builder_uid)
}

/// Whether `uid` is one [`mint_uid`] makes.
pub fn is_builder_uid(uid: &str) -> bool {
    uid.strip_prefix(UID_PREFIX).is_some_and(|rest| {
        rest.len() == 16
            && rest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// A new dashboard's uid: 64 random bits after [`UID_PREFIX`], within Grafana's 40 characters.
pub fn mint_uid() -> String {
    format!("{UID_PREFIX}{}", &crate::grafana::new_id()[..16])
}

/// The dashboard a file at `uid` holds, as the sync provisions it; `Err` names why it is refused.
pub fn read_file(uid: &str, text: &str) -> Result<Value, String> {
    if text.len() > MAX_FILE {
        return Err(format!("the file is over {} KiB", MAX_FILE / 1024));
    }
    let file: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let object = file.as_object().ok_or("not a JSON object")?;
    if let Some(stray) = object.keys().find(|key| !FILE_KEYS.contains(&key.as_str())) {
        return Err(format!("a member the builder does not write: {stray}"));
    }
    if file["uid"] != uid {
        return Err(format!("its uid is not the file's name {uid}"));
    }
    if file["panels"]
        .as_array()
        .is_some_and(|p| p.len() > MAX_PANELS)
    {
        return Err(format!("over {MAX_PANELS} panels"));
    }
    builders_own(&file)?;
    Ok(file)
}

const PANEL_KEYS: [&str; 9] = [
    "id",
    "type",
    "title",
    "description",
    "gridPos",
    "datasource",
    "targets",
    "fieldConfig",
    "options",
];

const TARGET_KEYS: [&str; 12] = [
    "refId",
    "datasource",
    "entityType",
    "attributes",
    "limit",
    "queryType",
    "format",
    "aggrMethod",
    "aggrPeriodDuration",
    "entityName",
    "namePattern",
    "customQuery",
];

const VARIABLE_KEYS: [&str; 12] = [
    "name",
    "label",
    "type",
    "description",
    "query",
    "multi",
    "includeAll",
    "allValue",
    "current",
    "options",
    "hide",
    "skipUrlSync",
];

/// Whether every panel and variable is one the builder writes: an NGSI-LD panel of the four
/// kinds with one target of the builder's members and `q` clauses, a custom variable. A file
/// that came another way (an import, a push to the branch) is refused, so no other query and no
/// other datasource reaches the dashboards service from the repository.
fn builders_own(file: &Value) -> Result<(), String> {
    let stray = |value: &Value, keys: &[&str]| {
        value.as_object().and_then(|object| {
            object
                .keys()
                .find(|key| !keys.contains(&key.as_str()))
                .cloned()
        })
    };
    for panel in file["panels"].as_array().into_iter().flatten() {
        let id = &panel["id"];
        if let Some(key) = stray(panel, &PANEL_KEYS) {
            return Err(format!(
                "panel {id} has a member the builder does not write: {key}"
            ));
        }
        if PanelKind::parse(panel["type"].as_str().unwrap_or_default()).is_none() {
            return Err(format!(
                "panel {id} is not a time series, stat, table or gauge"
            ));
        }
        if panel["datasource"]["type"] != PLUGIN {
            return Err(format!("panel {id} reads no NGSI-LD Endpoint"));
        }
        let targets = panel["targets"].as_array().map_or(&[][..], Vec::as_slice);
        let [target] = targets else {
            return Err(format!(
                "panel {id} has {} queries; the builder writes one",
                targets.len()
            ));
        };
        if let Some(key) = stray(target, &TARGET_KEYS) {
            return Err(format!(
                "panel {id}'s query has a member the builder does not write: {key}"
            ));
        }
        if target["datasource"] != panel["datasource"] {
            return Err(format!(
                "panel {id}'s query reads another datasource than its panel"
            ));
        }
        if let Some(q) = target["customQuery"].as_str() {
            if q.split(';').any(|clause| filter_of(clause).is_none()) {
                return Err(format!(
                    "panel {id} filters by text the builder does not write"
                ));
            }
        }
    }
    for variable in file["templating"]["list"].as_array().into_iter().flatten() {
        if let Some(key) = stray(variable, &VARIABLE_KEYS) {
            return Err(format!(
                "a variable has a member the builder does not write: {key}"
            ));
        }
        if variable["type"] != "custom"
            || !variable_name_ok(variable["name"].as_str().unwrap_or_default())
        {
            return Err(format!(
                "variable {} is not one the builder writes",
                variable["name"]
            ));
        }
    }
    Ok(())
}

/// The members two copies of one dashboard must agree on: the service adds `id` and `version`.
pub fn meaning(dashboard: &Value) -> Value {
    let mut kept = serde_json::Map::new();
    for key in FILE_KEYS {
        if let Some(value) = dashboard.get(key) {
            kept.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(kept)
}

/// The dashboard files of a staged repository: `(project, uid, text)` of each
/// `projects/{project}/grafana/{uid}.json`.
pub fn files_in(root: &Path) -> Vec<(String, String, String)> {
    let mut found = Vec::new();
    let Ok(projects) = std::fs::read_dir(root.join("projects")) else {
        return found;
    };
    for project in projects.flatten() {
        let Ok(files) = std::fs::read_dir(project.path().join(DIR)) else {
            continue;
        };
        for file in files.flatten() {
            let path = format!(
                "projects/{}/{DIR}/{}",
                project.file_name().to_string_lossy(),
                file.file_name().to_string_lossy()
            );
            let Some((project, uid)) = parse_path(&path) else {
                continue;
            };
            if let Ok(text) = std::fs::read_to_string(file.path()) {
                found.push((project.to_owned(), uid.to_owned(), text));
            }
        }
    }
    found.sort();
    found
}

/// What one sync pass did to one dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Saved(String, String),
    Kept(String, String),
    Removed(String, String),
    Refused(String, String, String),
}

/// The repository's approved dashboard files, provisioned into the dashboards service on each
/// reconciler run of the leader: a new or changed file is saved into the project's folder, a
/// builder dashboard no file holds any more is removed, only in a project whose repository staged.
pub struct DashboardSync {
    config: GrafanaConfig,
}

impl DashboardSync {
    pub fn new(config: GrafanaConfig) -> Self {
        Self { config }
    }

    pub async fn converge(
        &self,
        root: &Path,
        projects: &[String],
        unstaged: &[String],
    ) -> Vec<Outcome> {
        let client = Client::new(&self.config);
        let files = files_in(root);
        let mut outcomes = Vec::new();
        let mut touched: BTreeSet<String> = files.iter().map(|(p, _, _)| p.clone()).collect();
        touched.extend(projects.iter().cloned());
        for project in touched {
            let org = match client.organization(&project).await {
                Ok(Some(org)) => org,
                Ok(None) if client.derives_organizations().await => {
                    match client.provision(&project).await {
                        Ok(org) => org,
                        Err(_) => continue,
                    }
                }
                // A project the service holds no organization for has nothing to provision into.
                Ok(None) => continue,
                // A service that is away fails each project alike: the next run tries again.
                Err(_) => return outcomes,
            };
            let mine: Vec<&(String, String, String)> =
                files.iter().filter(|(p, _, _)| *p == project).collect();
            for (_, uid, text) in &mine {
                outcomes.push(self.provision_one(&client, org, &project, uid, text).await);
            }
            if unstaged.contains(&project) {
                continue;
            }
            let Ok(held) = client.dashboards(org).await else {
                continue;
            };
            for hit in held {
                let Some(uid) = hit["uid"].as_str().filter(|uid| is_builder_uid(uid)) else {
                    continue;
                };
                if mine.iter().any(|(_, held, _)| held == uid) {
                    continue;
                }
                if client.remove(org, uid).await.is_ok() {
                    tracing::info!(project = %project, uid = %uid, "a dashboard no file holds was removed from the dashboards service");
                    outcomes.push(Outcome::Removed(project.clone(), uid.to_owned()));
                }
            }
        }
        outcomes
    }

    async fn provision_one(
        &self,
        client: &Client<'_>,
        org: i64,
        project: &str,
        uid: &str,
        text: &str,
    ) -> Outcome {
        let file = match read_file(uid, text) {
            Ok(file) => file,
            Err(why) => {
                tracing::warn!(project = %project, uid = %uid, why = %why, "a dashboard file was refused");
                return Outcome::Refused(project.into(), uid.into(), why);
            }
        };
        if let Ok(Some(held)) = client.dashboard(org, uid).await {
            if meaning(&held) == meaning(&file) {
                return Outcome::Kept(project.into(), uid.into());
            }
        }
        match client.save(org, &file, project).await {
            Ok(()) => {
                tracing::info!(project = %project, uid = %uid, "a dashboard file was provisioned");
                Outcome::Saved(project.into(), uid.into())
            }
            Err(err) => Outcome::Refused(project.into(), uid.into(), err.to_string()),
        }
    }
}

/// The picked values of each variable as Grafana's `${name:regex}` writes them for a multi-value
/// variable, for a preview the service runs with no saved dashboard: each value escaped, several
/// as `(a|b)`, All as `.*`.
pub fn regex_values(picked: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, String> {
    let escape = |value: &str| {
        let mut out = String::with_capacity(value.len());
        for c in value.chars() {
            if "\\$^*{}[]'+?.()|".contains(c) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    };
    picked
        .iter()
        .map(|(name, values)| {
            let text = if values.iter().any(|v| v == "$__all") || values.is_empty() {
                ".*".to_owned()
            } else if let [one] = values.as_slice() {
                escape(one)
            } else {
                format!(
                    "({})",
                    values
                        .iter()
                        .map(|v| escape(v))
                        .collect::<Vec<_>>()
                        .join("|")
                )
            };
            (name.clone(), text)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn panel(id: u32, kind: PanelKind, x: u32) -> BoardPanel {
        BoardPanel {
            id,
            kind,
            title: format!("Panel {id}"),
            description: String::new(),
            grid: Grid {
                x,
                y: 0,
                w: 6,
                h: 8,
            },
            endpoint: "open".into(),
            entity_type: "AirQualityObserved".into(),
            attribute: "pm25".into(),
            aggregation: Aggregation::None,
            interval: Interval::Auto,
            filters: Vec::new(),
            unit: String::new(),
            decimals: None,
            thresholds: None,
        }
    }

    pub(crate) fn board() -> Board {
        let mut series = panel(1, PanelKind::Timeseries, 0);
        series.aggregation = Aggregation::Avg;
        series.interval = Interval::Hour;
        series.unit = "conμgm3".into();
        series.decimals = Some(1);
        series.thresholds = Some(Thresholds {
            base: "green".into(),
            steps: vec![ThresholdStep {
                value: 25.0,
                color: "red".into(),
            }],
        });
        series.filters = vec![
            Filter {
                attribute: "pm25".into(),
                op: Op::Gt,
                value: Some("5".into()),
                variable: None,
            },
            Filter {
                attribute: "station".into(),
                op: Op::Eq,
                value: None,
                variable: Some("station".into()),
            },
        ];
        let mut stat = panel(2, PanelKind::Stat, 6);
        stat.aggregation = Aggregation::Max;
        let table = panel(3, PanelKind::Table, 12);
        let mut gauge = panel(4, PanelKind::Gauge, 18);
        gauge.filters = vec![Filter {
            attribute: "station".into(),
            op: Op::Ne,
            value: Some("Kallio".into()),
            variable: None,
        }];
        Board {
            title: "Air".into(),
            description: "Air in Helsinki".into(),
            from: "now-24h".into(),
            to: "now".into(),
            refresh: "5m".into(),
            variables: vec![BoardVariable {
                name: "station".into(),
                label: "Station".into(),
                endpoint: "open".into(),
                entity_type: "AirQualityObserved".into(),
                attribute: "station".into(),
                include_all: true,
            }],
            panels: vec![series, stat, table, gauge],
        }
    }

    pub(crate) fn sources(board: &Board) -> Vec<Source> {
        board
            .panels
            .iter()
            .map(|p| Source {
                datasource: "open".into(),
                named: true,
                filters: p
                    .filters
                    .iter()
                    .map(|f| {
                        if f.attribute == "pm25" {
                            ValueKind::Number
                        } else {
                            ValueKind::Text
                        }
                    })
                    .collect(),
            })
            .collect()
    }

    #[test]
    fn a_board_compiles_into_grafana_json_and_reads_back_equal() {
        let board = board();
        assert_eq!(check_shape(&board), Ok(()));
        let file = compile(
            "jcd-0123456789abcdef",
            &board,
            &sources(&board),
            &[vec!["Kallio".into(), "Vallila".into()]],
        );
        let back = decompile(&file, &|uid| (uid == "open").then(|| "open".to_owned()))
            .expect("the builder's own file");
        assert_eq!(back, board);
        let target = &file["panels"][0]["targets"][0];
        assert_eq!(target["queryType"], "temporal");
        assert_eq!(target["aggrMethod"], "avg");
        assert_eq!(target["aggrPeriodDuration"], "PT1H");
        assert_eq!(
            target["customQuery"],
            "pm25>5;station~=\"^${station:regex}$\""
        );
        assert_eq!(
            file["panels"][1]["targets"][0]["queryType"], "temporal",
            "a stat with an aggregation reads the range"
        );
        assert_eq!(
            file["panels"][1]["options"]["reduceOptions"]["calcs"],
            json!(["max"])
        );
        assert_eq!(file["panels"][2]["targets"][0]["format"], "table");
        assert_eq!(
            file["panels"][3]["targets"][0]["customQuery"],
            "station!=\"Kallio\""
        );
        let variable = &file["templating"]["list"][0];
        assert_eq!(variable["query"], "Kallio,Vallila");
        assert_eq!(variable["allValue"], ".*");
        assert_eq!(
            read_file("jcd-0123456789abcdef", &file.to_string()),
            Ok(file.clone())
        );
    }

    /// One way to break a board, and the field its refusal names.
    type Breaks = Vec<(&'static str, Box<dyn Fn(&mut Board)>)>;

    #[test]
    fn every_shape_refusal_names_its_field() {
        let cases: Breaks = vec![
            ("title", Box::new(|b| b.title = " ".into())),
            ("title", Box::new(|b| b.title = "x".repeat(MAX_TITLE + 1))),
            ("from", Box::new(|b| b.from = "yesterday".into())),
            ("refresh", Box::new(|b| b.refresh = "5s".into())),
            (
                "variables[0].name",
                Box::new(|b| b.variables[0].name = "__all".into()),
            ),
            (
                "panels",
                Box::new(|b| b.panels = (1..=25).map(|i| panel(i, PanelKind::Stat, 0)).collect()),
            ),
            ("panels[1].grid", Box::new(|b| b.panels[1].grid.x = 3)),
            ("panels[0].grid", Box::new(|b| b.panels[0].grid.w = 25)),
            ("panels[1].id", Box::new(|b| b.panels[1].id = 1)),
            (
                "panels[2].aggregation",
                Box::new(|b| b.panels[2].aggregation = Aggregation::Avg),
            ),
            (
                "panels[2].interval",
                Box::new(|b| b.panels[2].interval = Interval::Day),
            ),
            (
                "panels[0].unit",
                Box::new(|b| b.panels[0].unit = "furlongs".into()),
            ),
            (
                "panels[0].decimals",
                Box::new(|b| b.panels[0].decimals = Some(9)),
            ),
            (
                "panels[0].thresholds.steps[0].color",
                Box::new(|b| {
                    b.panels[0].thresholds.as_mut().unwrap().steps[0].color = "#ff0000".into()
                }),
            ),
            (
                "panels[0].filters[1].variable",
                Box::new(|b| b.panels[0].filters[1].variable = Some("zone".into())),
            ),
            (
                "panels[3].filters[0].value",
                Box::new(|b| b.panels[3].filters[0].value = Some("a\" | x".into())),
            ),
            (
                "panels[0].endpoint",
                Box::new(|b| b.panels[0].endpoint = "../other".into()),
            ),
        ];
        for (field, change) in cases {
            let mut board = board();
            change(&mut board);
            let refusal = check_shape(&board).expect_err(field);
            assert_eq!(refusal.field, field, "{refusal:?}");
        }
    }

    #[test]
    fn a_board_with_a_query_member_does_not_parse() {
        let mut sent = serde_json::to_value(board()).expect("a board");
        sent["panels"][0]["targets"] = json!([{ "customQuery": "id~=.*" }]);
        assert!(serde_json::from_value::<Board>(sent).is_err());
        let mut sent = serde_json::to_value(board()).expect("a board");
        sent["panels"][0]["customQuery"] = json!("pm25>0");
        assert!(serde_json::from_value::<Board>(sent).is_err());
    }

    #[test]
    fn a_file_the_builder_did_not_write_is_not_read_back_as_choices() {
        let board = board();
        let file = compile("jcd-0123456789abcdef", &board, &sources(&board), &[vec![]]);
        let mut extra = file.clone();
        extra["panels"][0]["targets"][0]["customQuery"] = json!("pm25>5|id~=.*");
        assert!(decompile(&extra, &|_| Some("open".into())).is_err());
        let mut foreign = file.clone();
        foreign["folderUid"] = json!("project-other");
        assert!(decompile(&foreign, &|_| Some("open".into())).is_err());
        assert!(read_file("jcd-0123456789abcdef", &foreign.to_string()).is_err());
        assert!(
            read_file("jcd-ffffffffffffffff", &file.to_string()).is_err(),
            "a uid that is not the file's"
        );
        assert!(
            decompile(&file, &|_| None).is_err(),
            "a datasource of no Endpoint"
        );
        for (field, value) in [
            ("/panels/0/datasource/type", json!("prometheus")),
            ("/panels/0/targets/0/expr", json!("up")),
            ("/panels/0/transformations", json!([])),
            ("/panels/0/type", json!("geomap")),
            ("/templating/list/0/type", json!("query")),
        ] {
            let mut other = file.clone();
            let (parent, key) = field.rsplit_once('/').expect("a path");
            other.pointer_mut(parent).expect("a member")[key] = value;
            if key == "type" && parent == "/panels/0/datasource" {
                other["panels"][0]["targets"][0]["datasource"]["type"] = json!("prometheus");
            }
            assert!(
                read_file("jcd-0123456789abcdef", &other.to_string()).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn only_the_builders_own_paths_and_uids_are_dashboard_files() {
        assert_eq!(
            parse_path("projects/helsinki/grafana/jcd-0123456789abcdef.json"),
            Some(("helsinki", "jcd-0123456789abcdef"))
        );
        for path in [
            "projects/helsinki/grafana/pv-0123456789abcdef.json",
            "projects/helsinki/grafana/jcd-0123456789abcdef.yaml",
            "projects/Helsinki/grafana/jcd-0123456789abcdef.json",
            "projects/a/b/grafana/jcd-0123456789abcdef.json",
            "projects/helsinki/dashboards/jcd-0123456789abcdef.json",
        ] {
            assert_eq!(parse_path(path), None, "{path}");
        }
        assert!(is_dashboard_file(
            "projects/_/grafana/jcd-0123456789abcdef.json"
        ));
        assert!(!is_dashboard_file(
            "projects/../grafana/jcd-0123456789abcdef.json"
        ));
        assert!(!is_dashboard_file("projects/a/grafana/notes.json"));
        let uid = mint_uid();
        assert!(is_builder_uid(&uid) && uid.len() <= 40, "{uid}");
    }

    #[test]
    fn previews_write_picks_as_grafana_writes_a_regex_variable() {
        let picked = BTreeMap::from([
            ("a".to_owned(), vec!["$__all".to_owned()]),
            ("b".to_owned(), vec!["Kallio".to_owned()]),
            ("c".to_owned(), vec!["x.y".to_owned(), "z".to_owned()]),
        ]);
        let out = regex_values(&picked);
        assert_eq!(out["a"], ".*");
        assert_eq!(out["b"], "Kallio");
        assert_eq!(out["c"], "(x\\.y|z)");
        assert_eq!(
            offered(["b".into(), "a".into(), "a".into(), "x\"y".into(), "".into()]),
            ["a", "b"]
        );
    }
}
