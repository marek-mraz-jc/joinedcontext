#![forbid(unsafe_code)]
//! `FrameSet` data model (§3.2): columnar frames shared by datasources, expressions,
//! transformations, OData responses, and the Portal / dataroom views.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Columnar field type (§3.2: `time|number|string|bool|json|geo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Time,
    Number,
    String,
    Bool,
    Json,
    Geo,
}

/// Columnar vector of nullable values matching [`FieldType`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ColumnValues {
    /// Epoch milliseconds for `FieldType::Time`.
    Time(Vec<Option<i64>>),
    /// Double-precision floats for `FieldType::Number`.
    Number(Vec<Option<f64>>),
    /// UTF-8 strings for `FieldType::String` or `FieldType::Geo` (WKT/GeoJSON).
    String(Vec<Option<String>>),
    /// Booleans for `FieldType::Bool`.
    Bool(Vec<Option<bool>>),
    /// Arbitrary JSON values for `FieldType::Json` or mixed columns.
    Json(Vec<Value>),
}

impl ColumnValues {
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Time(v) => v.len(),
            Self::Number(v) => v.len(),
            Self::String(v) => v.len(),
            Self::Bool(v) => v.len(),
            Self::Json(v) => v.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reads the value at `idx` as a `serde_json::Value`.
    #[must_use]
    pub fn get_json(&self, idx: usize) -> Value {
        match self {
            Self::Time(v) => v.get(idx).and_then(|x| *x).map_or(Value::Null, Value::from),
            Self::Number(v) => v
                .get(idx)
                .and_then(|x| *x)
                .and_then(serde_json::Number::from_f64)
                .map_or(Value::Null, Value::Number),
            Self::String(v) => v
                .get(idx)
                .and_then(|x| x.clone())
                .map_or(Value::Null, Value::String),
            Self::Bool(v) => v.get(idx).and_then(|x| *x).map_or(Value::Null, Value::Bool),
            Self::Json(v) => v.get(idx).cloned().unwrap_or(Value::Null),
        }
    }

    /// Reads the value at `idx` as `Option<f64>` when numeric.
    #[must_use]
    pub fn get_f64(&self, idx: usize) -> Option<f64> {
        match self {
            Self::Number(v) => v.get(idx).copied().flatten(),
            Self::Time(v) => v.get(idx).copied().flatten().map(|t| t as f64),
            Self::Bool(v) => v
                .get(idx)
                .copied()
                .flatten()
                .map(|b| if b { 1.0 } else { 0.0 }),
            Self::Json(v) => v.get(idx).and_then(Value::as_f64),
            Self::String(v) => v
                .get(idx)
                .and_then(|s| s.as_deref())
                .and_then(|s| s.parse::<f64>().ok()),
        }
    }

    /// Reads the value at `idx` as `Option<i64>` when time or integer.
    #[must_use]
    pub fn get_i64(&self, idx: usize) -> Option<i64> {
        match self {
            Self::Time(v) => v.get(idx).copied().flatten(),
            Self::Number(v) => v.get(idx).copied().flatten().map(|f| f as i64),
            Self::Json(v) => v.get(idx).and_then(Value::as_i64),
            Self::String(v) => v
                .get(idx)
                .and_then(|s| s.as_deref())
                .and_then(|s| s.parse::<i64>().ok()),
            Self::Bool(v) => v.get(idx).copied().flatten().map(i64::from),
        }
    }
}

/// A number cell: JSON has no NaN or infinity, so they travel as the words [`FieldColumn::f64_at`]
/// reads back (`NaN`, `+Inf`, `-Inf`), as Grafana's frames keep them apart from a null.
/// tests/grafana_parity.rs (daiya-expr) holds every expression to Grafana's answers.
#[must_use]
pub fn number_value(v: Option<f64>) -> Value {
    match v {
        None => Value::Null,
        Some(x) if x.is_nan() => Value::String("NaN".into()),
        Some(x) if x == f64::INFINITY => Value::String("+Inf".into()),
        Some(x) if x == f64::NEG_INFINITY => Value::String("-Inf".into()),
        // `-0` reads as Grafana writes it: 0.
        Some(x) => serde_json::Number::from_f64(if x == 0.0 { 0.0 } else { x })
            .map_or(Value::Null, Value::Number),
    }
}

/// One column in a [`Frame`] (§3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub values: Vec<Value>,
}

impl FieldColumn {
    /// Creates a time column from epoch millisecond timestamps.
    #[must_use]
    pub fn time(name: impl Into<String>, times_ms: Vec<i64>) -> Self {
        Self {
            name: name.into(),
            field_type: FieldType::Time,
            unit: None,
            display_name: None,
            labels: BTreeMap::new(),
            values: times_ms.into_iter().map(Value::from).collect(),
        }
    }

    /// Creates a nullable float64 number column.
    #[must_use]
    pub fn numbers(
        name: impl Into<String>,
        labels: BTreeMap<String, String>,
        vals: Vec<Option<f64>>,
    ) -> Self {
        Self {
            name: name.into(),
            field_type: FieldType::Number,
            unit: None,
            display_name: None,
            labels,
            values: vals.into_iter().map(number_value).collect(),
        }
    }

    /// Creates a string column.
    #[must_use]
    pub fn strings(name: impl Into<String>, vals: Vec<Option<String>>) -> Self {
        Self {
            name: name.into(),
            field_type: FieldType::String,
            unit: None,
            display_name: None,
            labels: BTreeMap::new(),
            values: vals
                .into_iter()
                .map(|v| v.map_or(Value::Null, Value::String))
                .collect(),
        }
    }

    /// Number of rows in this column.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Extracts the value at `idx` as `Option<f64>` (handling JSON numbers and `"NaN"`/`"Inf"`).
    #[must_use]
    pub fn f64_at(&self, idx: usize) -> Option<f64> {
        match self.values.get(idx)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) if s == "NaN" => Some(f64::NAN),
            Value::String(s) if s == "Inf" || s == "+Inf" => Some(f64::INFINITY),
            Value::String(s) if s == "-Inf" => Some(f64::NEG_INFINITY),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    /// Extracts the value at `idx` as `Option<i64>` (epoch ms).
    #[must_use]
    pub fn time_ms_at(&self, idx: usize) -> Option<i64> {
        match self.values.get(idx)? {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.timestamp_millis()),
            _ => None,
        }
    }

    /// Effective display name (`display_name` if set, else `name`).
    #[must_use]
    pub fn effective_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }
}

/// Severity of a [`Notice`] attached to [`FrameMeta`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NoticeSeverity {
    Info,
    Warning,
    Error,
}

/// Diagnostic or informational notice on a frame (§3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub severity: NoticeSeverity,
    pub text: String,
}

/// Metadata attached to a [`Frame`] (§3.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FrameMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executed_query_string: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<Notice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_visualisation_type: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom: BTreeMap<String, Value>,
}

/// One columnar data frame (§3.2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
    #[serde(default)]
    pub fields: Vec<FieldColumn>,
    #[serde(default)]
    pub meta: FrameMeta,
}

impl Frame {
    /// Number of rows in the frame (length of the first field, or 0).
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.fields.first().map_or(0, FieldColumn::len)
    }

    /// Strips sensitive metadata (`executed_query_string` and `custom`) for public dashboards (R7).
    pub fn sanitize_for_public(&mut self, show_query_text: bool) {
        if !show_query_text {
            self.meta.executed_query_string = None;
        }
        self.meta.custom.clear();
    }
}

/// The result of one `refId` query or expression (§3.2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FrameSet {
    pub ref_id: String,
    pub frames: Vec<Frame>,
}

impl FrameSet {
    #[must_use]
    pub fn new(ref_id: impl Into<String>, frames: Vec<Frame>) -> Self {
        Self {
            ref_id: ref_id.into(),
            frames,
        }
    }

    /// Sanitizes all frames in this set for anonymous public dashboard viewers (R7).
    pub fn sanitize_for_public(&mut self, show_query_text: bool) {
        for frame in &mut self.frames {
            frame.sanitize_for_public(show_query_text);
        }
    }
}
