// SPDX-License-Identifier: EUPL-1.2
//! Entity type summary profiling (T-4680): pure functions with no I/O
//! to compute per-attribute statistics, rank attributes, extract sample rows,
//! and format a compact textual and JSON representation within a 6,000-character cap.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Reason why row loading stopped for this profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BasisReason {
    /// All available rows were loaded.
    All,
    /// Hit the configured row limit.
    RowLimit,
    /// Hit the memory limit.
    MemoryLimit,
    /// Server is busy.
    Busy,
    /// Server is in tight resource state.
    Tight,
}

impl BasisReason {
    /// String identifier for structured representation.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            BasisReason::All => "All",
            BasisReason::RowLimit => "RowLimit",
            BasisReason::MemoryLimit => "MemoryLimit",
            BasisReason::Busy => "Busy",
            BasisReason::Tight => "Tight",
        }
    }
}

/// The basis of the summary profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Basis {
    /// Count of loaded rows.
    pub loaded: usize,
    /// Total row count if reported by broker.
    pub total: Option<u64>,
    /// The reason why loading finished.
    pub reason: BasisReason,
    /// Rows skipped due to size limits.
    pub skipped: usize,
}

/// Formatted sample of an entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SampleRow {
    /// The Entity `id`.
    pub id: String,
    /// Optional Entity name.
    pub name: Option<String>,
    /// Formatted attribute name-value pairs.
    pub values: Vec<(String, String)>,
}

/// Summary statistics of an individual attribute.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AttrStat {
    /// Numeric attribute statistics.
    Number {
        /// Attribute name.
        name: String,
        /// Number of non-null observations.
        n: usize,
        /// Count of null observations.
        nulls: usize,
        /// Minimum value.
        min: f64,
        /// Maximum value.
        max: f64,
        /// Arithmetic mean.
        mean: f64,
        /// Median value.
        median: f64,
        /// Standard deviation.
        stddev: f64,
        /// 10th percentile.
        p10: f64,
        /// 90th percentile.
        p90: f64,
        /// Histogram buckets: `(start, end, count)`.
        hist: Vec<(f64, f64, usize)>,
        /// Count of outliers.
        outliers: usize,
        /// Optional unit code.
        unit_code: Option<String>,
        /// Top 3 integer values if all values are integers.
        top3: Option<Vec<(i64, usize)>>,
    },
    /// Boolean attribute statistics.
    Boolean {
        /// Attribute name.
        name: String,
        /// Number of observations.
        n: usize,
        /// Share of `true` values.
        true_share: f64,
    },
    /// Date or time attribute statistics.
    DateTime {
        /// Attribute name.
        name: String,
        /// Number of observations.
        n: usize,
        /// Minimum date string.
        min: String,
        /// Maximum date string.
        max: String,
        /// Median date string.
        median: String,
    },
    /// Categorical attribute statistics.
    Category {
        /// Attribute name.
        name: String,
        /// Number of observations.
        n: usize,
        /// Number of distinct categories.
        distinct: usize,
        /// Top 5 categories with count and percentage.
        top5: Vec<(String, usize, f64)>,
        /// Count of observations in categories outside top 5.
        other: usize,
    },
    /// Free text attribute statistics.
    Text {
        /// Attribute name.
        name: String,
        /// Number of observations.
        n: usize,
        /// Share of entities where this attribute is present.
        share_present: f64,
    },
    /// Geographic attribute statistics.
    Geo {
        /// Attribute name.
        name: String,
        /// Number of observations.
        n: usize,
        /// Bounding box: `[min_lon, min_lat, max_lon, max_lat]`.
        bbox: [f64; 4],
        /// Centroid: `[lon, lat]`.
        centroid: [f64; 2],
    },
    /// Relationship attribute statistics.
    Relationship {
        /// Attribute name.
        name: String,
        /// Number of observations.
        n: usize,
        /// Number of distinct target entities.
        distinct: usize,
        /// Top 3 target entities with occurrence counts.
        top3: Vec<(String, usize)>,
    },
    /// Structured attribute statistics.
    Structured {
        /// Attribute name.
        name: String,
        /// Number of observations.
        n: usize,
        /// Share of entities where this attribute is present.
        share_present: f64,
    },
}

impl AttrStat {
    /// Return the attribute name.
    pub(crate) fn name(&self) -> &str {
        match self {
            AttrStat::Number { name, .. }
            | AttrStat::Boolean { name, .. }
            | AttrStat::DateTime { name, .. }
            | AttrStat::Category { name, .. }
            | AttrStat::Text { name, .. }
            | AttrStat::Geo { name, .. }
            | AttrStat::Relationship { name, .. }
            | AttrStat::Structured { name, .. } => name,
        }
    }
}

/// The complete profile of an entity type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Profile {
    /// Entity type name.
    pub entity_type: String,
    /// Total count of entities if known.
    pub count: Option<u64>,
    /// The profile basis.
    pub basis: Basis,
    /// When the profile was computed.
    pub computed: chrono::DateTime<chrono::Utc>,
    /// Range of `observedAt` timestamps if present: `(min, max)`.
    pub observed_at: Option<(String, String)>,
    /// Top profiled attributes.
    pub attributes: Vec<AttrStat>,
    /// Names of attributes beyond the top ones.
    pub other_attributes: Vec<String>,
    /// Sample entities.
    pub samples: Vec<SampleRow>,
}

impl Profile {
    /// Convert the `Profile` to a JSON structure for tool `structuredContent`.
    pub(crate) fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("type".into(), json!(self.entity_type));
        if let Some(c) = self.count {
            map.insert("count".into(), json!(c));
        }
        map.insert(
            "basis".into(),
            json!({
                "loaded": self.basis.loaded,
                "total": self.basis.total,
                "reason": self.basis.reason.as_str(),
                "skipped": self.basis.skipped,
            }),
        );
        map.insert("computed".into(), json!(self.computed.to_rfc3339()));

        if self.basis.reason == BasisReason::Tight {
            map.insert(
                "unavailable".into(),
                json!("summary unavailable now, server busy"),
            );
            return Value::Object(map);
        }

        if let Some((min, max)) = &self.observed_at {
            map.insert(
                "observedAt".into(),
                json!({
                    "min": min,
                    "max": max,
                }),
            );
        }

        let mut attrs_json = serde_json::Map::new();
        for attr in &self.attributes {
            attrs_json.insert(attr.name().to_string(), attr_to_json(attr));
        }
        map.insert("attributes".into(), Value::Object(attrs_json));

        if !self.other_attributes.is_empty() {
            map.insert("otherAttributes".into(), json!(self.other_attributes));
        }

        let samples_json: Vec<Value> = self
            .samples
            .iter()
            .map(|s| {
                let mut row = serde_json::Map::new();
                row.insert("id".into(), json!(s.id));
                if let Some(name) = &s.name {
                    row.insert("name".into(), json!(name));
                }
                for (k, v) in &s.values {
                    row.insert(k.clone(), json!(v));
                }
                Value::Object(row)
            })
            .collect();
        map.insert("samples".into(), Value::Array(samples_json));

        Value::Object(map)
    }
}

/// A number attribute's statistics as JSON; any other kind is `null`.
fn number_to_json(attr: &AttrStat) -> Value {
    let AttrStat::Number {
        n,
        nulls,
        min,
        max,
        mean,
        median,
        stddev,
        p10,
        p90,
        hist,
        outliers,
        unit_code,
        top3,
        ..
    } = attr
    else {
        return Value::Null;
    };
    let (n, nulls, min, max, mean, median, stddev, p10, p90, outliers) = (
        *n, *nulls, *min, *max, *mean, *median, *stddev, *p10, *p90, *outliers,
    );
    let unit_code = unit_code.as_deref();
    let top3 = top3.as_deref();
    let hist_json: Vec<Value> = hist
        .iter()
        .map(|(s, e, c)| json!({"start": s, "end": e, "count": c}))
        .collect();
    let mut map = serde_json::Map::new();
    map.insert("kind".into(), json!("number"));
    map.insert("n".into(), json!(n));
    map.insert("nulls".into(), json!(nulls));
    map.insert("min".into(), json!(min));
    map.insert("max".into(), json!(max));
    map.insert("mean".into(), json!(mean));
    map.insert("median".into(), json!(median));
    map.insert("stddev".into(), json!(stddev));
    map.insert("p10".into(), json!(p10));
    map.insert("p90".into(), json!(p90));
    map.insert("hist".into(), Value::Array(hist_json));
    map.insert("outliers".into(), json!(outliers));
    if let Some(u) = unit_code {
        map.insert("unitCode".into(), json!(u));
    }
    if let Some(top) = top3 {
        let top_json: Vec<Value> = top
            .iter()
            .map(|(v, c)| json!({"value": v, "count": c}))
            .collect();
        map.insert("top3".into(), Value::Array(top_json));
    }
    Value::Object(map)
}

fn category_to_json(
    n: usize,
    distinct: usize,
    top5: &[(String, usize, f64)],
    other: usize,
) -> Value {
    let top_json: Vec<Value> = top5
        .iter()
        .map(|(v, c, pct)| json!({"value": v, "count": c, "pct": pct}))
        .collect();
    json!({
        "kind": "category",
        "n": n,
        "distinct": distinct,
        "top5": top_json,
        "other": other,
    })
}

fn relationship_to_json(n: usize, distinct: usize, top3: &[(String, usize)]) -> Value {
    let top_json: Vec<Value> = top3
        .iter()
        .map(|(v, c)| json!({"target": v, "count": c}))
        .collect();
    json!({
        "kind": "relationship",
        "n": n,
        "distinct": distinct,
        "top3": top_json,
    })
}

fn attr_to_json(attr: &AttrStat) -> Value {
    match attr {
        AttrStat::Number { .. } => number_to_json(attr),
        AttrStat::Boolean { n, true_share, .. } => json!({
            "kind": "boolean",
            "n": n,
            "trueShare": true_share,
        }),
        AttrStat::DateTime {
            n,
            min,
            max,
            median,
            ..
        } => json!({
            "kind": "dateTime",
            "n": n,
            "min": min,
            "max": max,
            "median": median,
        }),
        AttrStat::Category {
            n,
            distinct,
            top5,
            other,
            ..
        } => category_to_json(*n, *distinct, top5, *other),
        AttrStat::Text {
            n, share_present, ..
        } => json!({
            "kind": "text",
            "n": n,
            "sharePresent": share_present,
        }),
        AttrStat::Geo {
            n, bbox, centroid, ..
        } => json!({
            "kind": "geo",
            "n": n,
            "bbox": bbox,
            "centroid": centroid,
        }),
        AttrStat::Relationship {
            n, distinct, top3, ..
        } => relationship_to_json(*n, *distinct, top3),
        AttrStat::Structured {
            n, share_present, ..
        } => json!({
            "kind": "structured",
            "n": n,
            "sharePresent": share_present,
        }),
    }
}

/// The day of an RFC 3339 instant or of a `YYYY-MM-DD` date.
fn parse_rfc3339_or_date(s: &str) -> Option<String> {
    let valid = chrono::DateTime::parse_from_rfc3339(s).is_ok()
        || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok();
    valid.then(|| s.get(..10)).flatten().map(str::to_owned)
}

fn extract_coords(val: &Value, out: &mut Vec<(f64, f64)>) {
    if let Value::Array(arr) = val {
        if arr.len() >= 2 && arr.first().is_some_and(Value::is_number) {
            let lon = arr.first().and_then(Value::as_f64);
            let lat = arr.get(1).and_then(Value::as_f64);
            if let (Some(x), Some(y)) = (lon, lat) {
                out.push((x, y));
                return;
            }
        }
        for item in arr {
            extract_coords(item, out);
        }
    }
}

fn to_f64(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Each attribute's values across the rows, by name.
type Presences<'a> = BTreeMap<String, Vec<&'a Value>>;

fn collect_presences_and_dates(rows: &[Value]) -> (Presences<'_>, Option<(String, String)>) {
    let mut attr_presences: Presences<'_> = BTreeMap::new();
    let mut observed_ats: Vec<String> = Vec::new();

    for row in rows {
        let Some(map) = row.as_object() else { continue };
        for (k, v) in map {
            if matches!(k.as_str(), "id" | "type" | "@context" | "scope") {
                continue;
            }
            attr_presences.entry(k.clone()).or_default().push(v);
            if let Some(obs) = v
                .as_object()
                .and_then(|o| o.get("observedAt"))
                .and_then(Value::as_str)
            {
                if let Some(d) = parse_rfc3339_or_date(obs) {
                    observed_ats.push(d);
                }
            }
        }
    }

    let observed_at = if observed_ats.is_empty() {
        None
    } else {
        observed_ats.sort();
        let min = observed_ats.first().cloned().unwrap_or_default();
        let max = observed_ats.last().cloned().unwrap_or_default();
        Some((min, max))
    };

    (attr_presences, observed_at)
}

fn rank_attributes(presences: &BTreeMap<String, Vec<&Value>>) -> (Vec<String>, Vec<String>) {
    let mut attr_names: Vec<(String, usize)> = presences
        .iter()
        .map(|(k, v)| (k.clone(), v.len()))
        .collect();
    attr_names.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let top12: Vec<String> = attr_names.iter().take(12).map(|(k, _)| k.clone()).collect();
    let other: Vec<String> = attr_names.iter().skip(12).map(|(k, _)| k.clone()).collect();
    (top12, other)
}

fn build_sample_rows(
    rows: &[Value],
    sample_attrs: &[String],
    lang: Option<&str>,
) -> Vec<SampleRow> {
    let mut samples = Vec::new();
    for row in rows.iter().take(5) {
        let Some(map) = row.as_object() else { continue };
        let id = map
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let name = map.get("name").and_then(|v| resolve_str(v, lang)).map(|s| {
            if s.len() > 80 {
                let trimmed: String = s.chars().take(80).collect();
                trimmed
            } else {
                s
            }
        });

        let mut values = Vec::new();
        for key in sample_attrs {
            if let Some(val_node) = map.get(key) {
                let formatted = format_sample_value(val_node, lang);
                values.push((key.clone(), formatted));
            }
        }
        samples.push(SampleRow { id, name, values });
    }
    samples
}

/// Compute `Profile` over normalized NGSI-LD entities.
pub(crate) fn profile(
    rows: &[Value],
    total: Option<u64>,
    basis: Basis,
    lang: Option<&str>,
) -> Profile {
    let entity_type = rows
        .first()
        .and_then(|r| r.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    let computed = chrono::Utc::now();
    let total_loaded = rows.len();

    if basis.reason == BasisReason::Tight || total_loaded == 0 {
        return Profile {
            entity_type,
            count: total,
            basis,
            computed,
            observed_at: None,
            attributes: Vec::new(),
            other_attributes: Vec::new(),
            samples: Vec::new(),
        };
    }

    let (attr_presences, observed_at) = collect_presences_and_dates(rows);
    let (top12, other_attributes) = rank_attributes(&attr_presences);

    let attributes: Vec<AttrStat> = top12
        .iter()
        .filter_map(|name| {
            let values = attr_presences.get(name)?;
            Some(analyze_attribute(name, values, total_loaded, lang))
        })
        .collect();

    let sample_attrs: Vec<String> = attributes
        .iter()
        .filter(|a| {
            !matches!(
                a,
                AttrStat::Geo { .. } | AttrStat::Text { .. } | AttrStat::Structured { .. }
            )
        })
        .take(5)
        .map(|a| a.name().to_string())
        .collect();

    let samples = build_sample_rows(rows, &sample_attrs, lang);

    Profile {
        entity_type,
        count: total,
        basis,
        computed,
        observed_at,
        attributes,
        other_attributes,
        samples,
    }
}

fn resolve_str(node: &Value, lang: Option<&str>) -> Option<String> {
    let obj = node.as_object()?;
    if obj.get("type").and_then(Value::as_str) == Some("LanguageProperty") {
        if let Some(lang_map) = obj.get("languageMap").and_then(Value::as_object) {
            if let Some(target) = lang {
                if let Some(s) = lang_map.get(target).and_then(Value::as_str) {
                    return Some(s.to_string());
                }
            }
            if let Some((_, v)) = lang_map.iter().next() {
                if let Some(s) = v.as_str() {
                    return Some(s.to_string());
                }
            }
        }
    }
    if let Some(s) = obj.get("value").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    None
}

fn format_sample_value(node: &Value, lang: Option<&str>) -> String {
    if let Some(obj) = node.as_object() {
        if obj.get("type").and_then(Value::as_str) == Some("Relationship") {
            if let Some(obj_uri) = obj.get("object").and_then(Value::as_str) {
                return obj_uri.to_string();
            }
        }
        if let Some(s) = resolve_str(node, lang) {
            if s.len() > 80 {
                let trimmed: String = s.chars().take(80).collect();
                return trimmed;
            }
            return s;
        }
        if let Some(num) = obj.get("value").and_then(Value::as_f64) {
            return fmt_num(num);
        }
        if let Some(b) = obj.get("value").and_then(Value::as_bool) {
            return b.to_string();
        }
    }
    let text = node.to_string();
    if text.len() > 80 {
        let trimmed: String = text.chars().take(80).collect();
        trimmed
    } else {
        text
    }
}

fn analyze_relationship(name: &str, values: &[&Value]) -> Option<AttrStat> {
    let n = values.len();
    let rel_count = values
        .iter()
        .filter(|v| {
            v.as_object()
                .and_then(|o| o.get("type"))
                .and_then(Value::as_str)
                == Some("Relationship")
        })
        .count();
    if rel_count <= n.checked_div(2).unwrap_or(0) {
        return None;
    }

    let mut target_counts: BTreeMap<String, usize> = BTreeMap::new();
    for v in values {
        if let Some(obj) = v
            .as_object()
            .and_then(|o| o.get("object"))
            .and_then(Value::as_str)
        {
            let count = target_counts.entry(obj.to_string()).or_default();
            *count = count.saturating_add(1);
        }
    }
    let distinct = target_counts.len();
    let mut sorted: Vec<(String, usize)> = target_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let top3 = sorted.into_iter().take(3).collect();
    Some(AttrStat::Relationship {
        name: name.to_string(),
        n,
        distinct,
        top3,
    })
}

fn analyze_geo(name: &str, values: &[&Value]) -> Option<AttrStat> {
    let n = values.len();
    let geo_count = values
        .iter()
        .filter(|v| {
            v.as_object().is_some_and(|o| {
                o.get("type").and_then(Value::as_str) == Some("GeoProperty")
                    || o.get("value").is_some_and(|val| {
                        val.as_object()
                            .is_some_and(|vo| vo.contains_key("coordinates"))
                    })
            })
        })
        .count();
    if geo_count <= n.checked_div(2).unwrap_or(0) {
        return None;
    }

    let mut all_coords = Vec::new();
    let mut entity_centres = Vec::new();
    for v in values {
        let mut coords = Vec::new();
        // the GeoJSON's positions: `value` is the geometry object, never a bare array
        if let Some(geom) = v.pointer("/value/coordinates") {
            extract_coords(geom, &mut coords);
        }
        if !coords.is_empty() {
            let mut min_x = f64::INFINITY;
            let mut max_x = f64::NEG_INFINITY;
            let mut min_y = f64::INFINITY;
            let mut max_y = f64::NEG_INFINITY;
            for (x, y) in &coords {
                min_x = min_x.min(*x);
                max_x = max_x.max(*x);
                min_y = min_y.min(*y);
                max_y = max_y.max(*y);
                all_coords.push((*x, *y));
            }
            entity_centres.push((f64::midpoint(min_x, max_x), f64::midpoint(min_y, max_y)));
        }
    }
    let (bbox, centroid) = if all_coords.is_empty() {
        ([0.0, 0.0, 0.0, 0.0], [0.0, 0.0])
    } else {
        let mut min_x = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for (x, y) in &all_coords {
            min_x = min_x.min(*x);
            max_x = max_x.max(*x);
            min_y = min_y.min(*y);
            max_y = max_y.max(*y);
        }
        let count = to_f64(entity_centres.len());
        let c_x = entity_centres.iter().map(|(x, _)| *x).sum::<f64>() / count;
        let c_y = entity_centres.iter().map(|(_, y)| *y).sum::<f64>() / count;
        ([min_x, min_y, max_x, max_y], [c_x, c_y])
    };
    Some(AttrStat::Geo {
        name: name.to_string(),
        n,
        bbox,
        centroid,
    })
}

/// The `p` quantile of `sorted`, linear between its neighbours; 0 when empty.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let last = sorted.len().saturating_sub(1);
    let idx = p * to_f64(last);
    let k = (0..=last).rev().find(|&i| to_f64(i) <= idx).unwrap_or(0);
    let at = |i: usize| sorted.get(i).copied().unwrap_or(0.0);
    at(k) + (idx - to_f64(k)) * (at(k.saturating_add(1).min(last)) - at(k))
}

fn compute_histogram(
    numbers: &[f64],
    min: f64,
    max: f64,
    num_count: usize,
) -> Vec<(f64, f64, usize)> {
    let mut hist = Vec::new();
    if max > min {
        let width = (max - min) / 5.0;
        let bucket = |x: f64| {
            (0..5)
                .rev()
                .find(|&i| x >= min + to_f64(i) * width)
                .unwrap_or(0)
        };
        for i in 0..5 {
            let start = min + to_f64(i) * width;
            let end = if i == 4 {
                max
            } else {
                min + to_f64(i.saturating_add(1)) * width
            };
            let cnt = numbers.iter().filter(|&&x| bucket(x) == i).count();
            hist.push((start, end, cnt));
        }
    } else {
        hist.push((min, max, num_count));
    }
    hist
}

fn analyze_number(name: &str, values: &[&Value], total_loaded: usize) -> Option<AttrStat> {
    let n = values.len();
    let mut numbers = Vec::new();
    let mut integer_only = true;
    let mut unit_codes: Vec<String> = Vec::new();

    for v in values {
        if let Some(obj) = v.as_object() {
            if let Some(u) = obj.get("unitCode").and_then(Value::as_str) {
                if !u.is_empty() {
                    unit_codes.push(u.to_string());
                }
            }
            if let Some(num) = obj.get("value").and_then(Value::as_f64) {
                if num.fract() != 0.0 {
                    integer_only = false;
                }
                numbers.push(num);
            }
        }
    }

    if numbers.is_empty() || numbers.len() < n.checked_div(2).unwrap_or(0) {
        return None;
    }

    numbers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let num_count = numbers.len();
    let nulls = total_loaded.saturating_sub(num_count);
    let min = *numbers.first().unwrap_or(&0.0);
    let max = *numbers.last().unwrap_or(&0.0);
    let mean = numbers.iter().sum::<f64>() / to_f64(num_count);
    let median = percentile(&numbers, 0.5);

    let stddev = if num_count <= 1 {
        0.0
    } else {
        let var = numbers.iter().map(|x| (x - mean).powi(2)).sum::<f64>()
            / to_f64(num_count.saturating_sub(1));
        var.sqrt()
    };

    let p10 = percentile(&numbers, 0.10);
    let p90 = percentile(&numbers, 0.90);
    let q1 = percentile(&numbers, 0.25);
    let q3 = percentile(&numbers, 0.75);
    let iqr = q3 - q1;
    let lower = q1 - 1.5 * iqr;
    let upper = q3 + 1.5 * iqr;
    let outliers = numbers.iter().filter(|&&x| x < lower || x > upper).count();

    let hist = compute_histogram(&numbers, min, max, num_count);
    let unit_code = unit_codes
        .first()
        .filter(|first| unit_codes.iter().all(|u| u == *first))
        .cloned();

    let top3 = if integer_only {
        let mut freq: BTreeMap<i64, usize> = BTreeMap::new();
        for &num in &numbers {
            if let Ok(k) = format!("{num:.0}").parse::<i64>() {
                let c = freq.entry(k).or_default();
                *c = c.saturating_add(1);
            }
        }
        let mut sorted: Vec<(i64, usize)> = freq.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Some(sorted.into_iter().take(3).collect())
    } else {
        None
    };

    Some(AttrStat::Number {
        name: name.to_string(),
        n: num_count,
        nulls,
        min,
        max,
        mean,
        median,
        stddev,
        p10,
        p90,
        hist,
        outliers,
        unit_code,
        top3,
    })
}

fn analyze_boolean(name: &str, values: &[&Value]) -> Option<AttrStat> {
    let n = values.len();
    let mut bools = Vec::new();
    for v in values {
        if let Some(b) = v
            .as_object()
            .and_then(|o| o.get("value"))
            .and_then(Value::as_bool)
        {
            bools.push(b);
        }
    }
    if bools.is_empty() || bools.len() < n.checked_div(2).unwrap_or(0) {
        return None;
    }
    let true_count = bools.iter().filter(|&&b| b).count();
    let true_share = to_f64(true_count) / to_f64(bools.len());
    Some(AttrStat::Boolean {
        name: name.to_string(),
        n: bools.len(),
        true_share,
    })
}

fn analyze_datetime(name: &str, values: &[&Value]) -> Option<AttrStat> {
    let n = values.len();
    let mut dates = Vec::new();
    for v in values {
        // a plain string, or a typed `{"@type": "DateTime", "@value": …}`
        let value = v.get("value");
        let typed = value
            .filter(|x| x.get("@type").and_then(Value::as_str) == Some("DateTime"))
            .and_then(|x| x.get("@value"));
        if let Some(d) = value
            .and_then(Value::as_str)
            .or_else(|| typed.and_then(Value::as_str))
            .and_then(parse_rfc3339_or_date)
        {
            dates.push(d);
        }
    }
    if dates.is_empty() || dates.len() < n.checked_div(2).unwrap_or(0) {
        return None;
    }
    dates.sort();
    let min = dates.first().cloned().unwrap_or_default();
    let max = dates.last().cloned().unwrap_or_default();
    let median = dates
        .get(dates.len().checked_div(2).unwrap_or(0))
        .cloned()
        .unwrap_or_default();
    Some(AttrStat::DateTime {
        name: name.to_string(),
        n: dates.len(),
        min,
        max,
        median,
    })
}

fn analyze_category_or_text(
    name: &str,
    values: &[&Value],
    total_loaded: usize,
    lang: Option<&str>,
) -> Option<AttrStat> {
    let n = values.len();
    let mut strings = Vec::new();
    for v in values {
        if let Some(s) = resolve_str(v, lang) {
            strings.push(s);
        }
    }
    if strings.is_empty() || strings.len() < n.checked_div(2).unwrap_or(0) {
        return None;
    }

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for s in &strings {
        let c = counts.entry(s.clone()).or_default();
        *c = c.saturating_add(1);
    }
    let distinct = counts.len();
    if distinct <= 20 && distinct <= strings.len().checked_div(2).unwrap_or(0) {
        let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut top5 = Vec::new();
        let mut other = 0usize;
        for (idx, (val, cnt)) in sorted.into_iter().enumerate() {
            if idx < 5 {
                let pct = (to_f64(cnt) / to_f64(strings.len())) * 100.0;
                top5.push((val, cnt, pct));
            } else {
                other = other.saturating_add(cnt);
            }
        }
        return Some(AttrStat::Category {
            name: name.to_string(),
            n: strings.len(),
            distinct,
            top5,
            other,
        });
    }

    Some(AttrStat::Text {
        name: name.to_string(),
        n: strings.len(),
        share_present: to_f64(strings.len()) / to_f64(total_loaded),
    })
}

fn analyze_attribute(
    name: &str,
    values: &[&Value],
    total_loaded: usize,
    lang: Option<&str>,
) -> AttrStat {
    analyze_relationship(name, values)
        .or_else(|| analyze_geo(name, values))
        .or_else(|| analyze_number(name, values, total_loaded))
        .or_else(|| analyze_boolean(name, values))
        .or_else(|| analyze_datetime(name, values))
        .or_else(|| analyze_category_or_text(name, values, total_loaded, lang))
        .unwrap_or_else(|| AttrStat::Structured {
            name: name.to_string(),
            n: values.len(),
            share_present: to_f64(values.len()) / to_f64(total_loaded),
        })
}

/// Format a float to up to 3 significant figures, or whole for integers.
pub(crate) fn fmt_num(v: f64) -> String {
    if v.is_nan() || v.is_infinite() {
        return "0".into();
    }
    if v.fract() == 0.0 {
        return format!("{v:.0}");
    }
    let abs = v.abs();
    if abs >= 99.95 {
        format!("{v:.0}")
    } else if abs >= 9.995 {
        format!("{v:.1}")
    } else if abs >= 0.9995 {
        format!("{v:.2}")
    } else if abs >= 0.09995 {
        format!("{v:.3}")
    } else {
        format!("{v:.4}")
    }
}

fn render_basis(basis: &Basis) -> String {
    let total_str = basis
        .total
        .map_or_else(|| "unknown".to_string(), |t| t.to_string());
    let skipped_str = if basis.skipped > 0 {
        format!(", {} skipped (>1MiB)", basis.skipped)
    } else {
        String::new()
    };
    match basis.reason {
        BasisReason::All => format!("{} of {total_str} (all){skipped_str}", basis.loaded),
        BasisReason::RowLimit => {
            format!("{} of {total_str} (first by id){skipped_str}", basis.loaded)
        }
        BasisReason::MemoryLimit => format!(
            "{} of {total_str} (stopped by memory limit){skipped_str}",
            basis.loaded
        ),
        BasisReason::Busy => {
            format!("{} of {total_str} (server busy){skipped_str}", basis.loaded)
        }
        BasisReason::Tight => "summary unavailable now, server busy".to_string(),
    }
}

/// A number attribute's line; any other kind renders nothing.
fn render_number_stat(attr: &AttrStat) -> String {
    let AttrStat::Number {
        name,
        n,
        nulls,
        min,
        max,
        mean,
        median,
        stddev,
        p10,
        p90,
        hist,
        outliers,
        unit_code,
        top3,
        ..
    } = attr
    else {
        return String::new();
    };
    let (n, nulls, min, max, mean, median, stddev, p10, p90, outliers) = (
        *n, *nulls, *min, *max, *mean, *median, *stddev, *p10, *p90, *outliers,
    );
    let unit_code = unit_code.as_deref();
    let top3 = top3.as_deref();
    let mut parts = vec![format!("n={n}")];
    if nulls > 0 {
        parts.push(format!("nulls={nulls}"));
    }
    parts.push(format!("min={}", fmt_num(min)));
    parts.push(format!("max={}", fmt_num(max)));
    parts.push(format!("mean={}", fmt_num(mean)));
    parts.push(format!("median={}", fmt_num(median)));
    parts.push(format!("sd={}", fmt_num(stddev)));
    parts.push(format!("p10={}", fmt_num(p10)));
    parts.push(format!("p90={}", fmt_num(p90)));

    let hist_parts: Vec<String> = hist
        .iter()
        .map(|(s, e, c)| format!("{}-{}:{c}", fmt_num(*s), fmt_num(*e)))
        .collect();
    let hist_joined = hist_parts.join(",");
    parts.push(format!("hist=[{hist_joined}]"));

    if outliers > 0 {
        parts.push(format!("outliers={outliers}"));
    }
    if let Some(u) = unit_code {
        parts.push(format!("unitCode={u}"));
    }
    if let Some(top) = top3 {
        let top_parts: Vec<String> = top.iter().map(|(v, c)| format!("{v}:{c}")).collect();
        let top_joined = top_parts.join(",");
        parts.push(format!("top=[{top_joined}]"));
    }
    let joined = parts.join(" ");
    format!("{name}: {joined}")
}

fn render_category_stat(
    name: &str,
    distinct: usize,
    top5: &[(String, usize, f64)],
    other: usize,
) -> String {
    let top_parts: Vec<String> = top5
        .iter()
        .map(|(v, c, pct)| format!("{v} {c} {pct:.0}%"))
        .collect();
    let top_joined = top_parts.join(", ");
    let mut s = format!("{name}: distinct={distinct} top=[{top_joined}]");
    if other > 0 {
        let _ = write!(s, " other={other}");
    }
    s
}

fn render_stat(stat: &AttrStat) -> String {
    match stat {
        AttrStat::Number { .. } => render_number_stat(stat),
        AttrStat::Boolean {
            name,
            n,
            true_share,
        } => {
            format!("{name}: n={n} true={:.0}%", true_share * 100.0)
        }
        AttrStat::DateTime {
            name,
            n,
            min,
            max,
            median,
        } => {
            format!("{name}: n={n} min={min} max={max} median={median}")
        }
        AttrStat::Category {
            name,
            distinct,
            top5,
            other,
            ..
        } => render_category_stat(name, *distinct, top5, *other),
        AttrStat::Text {
            name,
            n,
            share_present,
        }
        | AttrStat::Structured {
            name,
            n,
            share_present,
        } => {
            format!("{name}: n={n} present={:.0}%", share_present * 100.0)
        }
        AttrStat::Geo {
            name,
            n,
            bbox: [b0, b1, b2, b3],
            centroid: [c0, c1],
        } => {
            format!(
                "{name}: n={n} bbox=[{}, {}, {}, {}] centroid=[{}, {}]",
                fmt_num(*b0),
                fmt_num(*b1),
                fmt_num(*b2),
                fmt_num(*b3),
                fmt_num(*c0),
                fmt_num(*c1)
            )
        }
        AttrStat::Relationship {
            name,
            n,
            distinct,
            top3,
        } => {
            let top_parts: Vec<String> = top3
                .iter()
                .map(|(target, count)| format!("{target} {count}"))
                .collect();
            let joined = top_parts.join(", ");
            format!("{name}: n={n} distinct={distinct} top=[{joined}]")
        }
    }
}

fn render_internal(profile: &Profile, include_samples: bool, full_attrs_count: usize) -> String {
    let mut lines = Vec::new();

    let count_val = profile
        .count
        .unwrap_or_else(|| u64::try_from(profile.basis.loaded).unwrap_or(u64::MAX));
    let computed_str = profile.computed.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    if profile.basis.reason == BasisReason::Tight {
        lines.push(format!(
            "type={} count={count_val} basis={} computed={computed_str}",
            profile.entity_type,
            render_basis(&profile.basis)
        ));
        return lines.join("\n");
    }

    if profile.basis.loaded == 0 {
        // Rows that exist but did not load (too large, past the budget) are
        // not an empty type (T-4683): the count stays the broker's.
        if count_val == 0 {
            lines.push(format!(
                "type={} count=0 basis=0 of 0 (all) computed={computed_str}",
                profile.entity_type
            ));
            lines.push("no entities found".to_string());
        } else {
            lines.push(format!(
                "type={} count={count_val} basis={} computed={computed_str}",
                profile.entity_type,
                render_basis(&profile.basis)
            ));
            lines.push("no entity could be loaded for statistics now".to_string());
        }
        return lines.join("\n");
    }

    lines.push(format!(
        "type={} count={count_val} basis={} computed={computed_str}",
        profile.entity_type,
        render_basis(&profile.basis)
    ));

    if let Some((min, max)) = &profile.observed_at {
        lines.push(format!("observedAt: min={min} max={max}"));
    }

    let full_stats: Vec<&AttrStat> = profile.attributes.iter().take(full_attrs_count).collect();
    let mut demoted_names: Vec<String> = profile
        .attributes
        .iter()
        .skip(full_attrs_count)
        .map(|a| a.name().to_string())
        .collect();
    demoted_names.extend(profile.other_attributes.clone());

    for stat in full_stats {
        lines.push(render_stat(stat));
    }

    if !demoted_names.is_empty() {
        lines.push(format!("otherAttributes: {}", demoted_names.join(", ")));
    }

    if include_samples && !profile.samples.is_empty() {
        lines.push("sampleEntities:".to_string());
        for sample in &profile.samples {
            let mut parts = Vec::new();
            if let Some(name) = &sample.name {
                parts.push(format!("name=\"{name}\""));
            }
            for (k, v) in &sample.values {
                parts.push(format!("{k}={v}"));
            }
            let id = &sample.id;
            let joined = parts.join(", ");
            lines.push(format!("  {id} ({joined})"));
        }
    }

    lines.join("\n")
}

/// Render the `Profile` to a terse textual form clamped to 6,000 characters.
pub(crate) fn render(profile: &Profile) -> String {
    let mut include_samples = true;
    let mut full_attrs_count = profile.attributes.len();

    loop {
        let text = render_internal(profile, include_samples, full_attrs_count);
        if text.len() <= 6000 {
            return text;
        }
        if include_samples {
            include_samples = false;
            continue;
        }
        if full_attrs_count > 0 {
            full_attrs_count = full_attrs_count.saturating_sub(1);
            continue;
        }
        // Last resort: truncate lines cleanly
        let mut truncated = Vec::new();
        let mut cur_len = 0usize;
        for line in text.lines() {
            let next_len = cur_len.saturating_add(line.len()).saturating_add(1);
            if next_len > 6000 {
                break;
            }
            truncated.push(line);
            cur_len = next_len;
        }
        return truncated.join("\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge_fixture() -> Vec<Value> {
        (0u32..10u32)
            .map(|i| {
                let year =
                    1900usize.saturating_add(usize::try_from(i).unwrap_or(0).saturating_mul(10));
                let day = 10u32.saturating_add(i);
                let fi = f64::from(i);
                json!({
                    "id": format!("urn:ngsi-ld:Bridge:{i}"),
                    "type": "Bridge",
                    "yearBuilt": {
                        "type": "Property",
                        "value": year,
                        "unitCode": "ANN"
                    },
                    "isOperational": {
                        "type": "Property",
                        "value": i % 2 == 0
                    },
                    "category": {
                        "type": "Property",
                        "value": if i < 6 { "cestny" } else { "zeleznicny" }
                    },
                    "description": {
                        "type": "Property",
                        "value": format!("Long description of bridge number {i}")
                    },
                    "dateModified": {
                        "type": "Property",
                        "value": format!("2026-10-{day:02}T10:00:00Z")
                    },
                    "location": {
                        "type": "GeoProperty",
                        "value": {
                            "type": "Point",
                            "coordinates": [17.10 + fi * 0.01, 48.14 + fi * 0.01]
                        }
                    },
                    "connectedTo": {
                        "type": "Relationship",
                        "object": if i < 7 { "urn:ngsi-ld:Road:1" } else { "urn:ngsi-ld:Road:2" }
                    }
                })
            })
            .collect()
    }

    /// `path` of `v`, `null` when it is not there.
    fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
        v.pointer(path).unwrap_or(&Value::Null)
    }

    #[test]
    fn bridge_statistics_are_exact() {
        let rows = bridge_fixture();
        let basis = Basis {
            loaded: rows.len(),
            total: Some(10),
            reason: BasisReason::All,
            skipped: 0,
        };
        let p = profile(&rows, Some(10), basis, None);
        assert_eq!(p.entity_type, "Bridge");
        let json = p.to_json();
        let year = at(&json, "/attributes/yearBuilt");
        let got: Vec<&Value> = ["n", "min", "max", "mean", "median", "unitCode"]
            .iter()
            .map(|k| at(year, &format!("/{k}")))
            .collect();
        assert_eq!(
            got,
            [
                &json!(10),
                &json!(1900.0),
                &json!(1990.0),
                &json!(1945.0),
                &json!(1945.0),
                &json!("ANN")
            ]
        );
        assert_eq!(at(&json, "/attributes/category/distinct"), &json!(2));
        assert_eq!(
            at(&json, "/attributes/category/top5/0"),
            &json!({"value": "cestny", "count": 6, "pct": 60.0})
        );
        assert_eq!(
            at(&json, "/attributes/connectedTo/top3/0"),
            &json!({"target": "urn:ngsi-ld:Road:1", "count": 7})
        );

        let text = render(&p);
        assert!(text.contains("type=Bridge"));
        assert!(text.contains("yearBuilt:"));
        // Text values in description should not appear
        assert!(!text.contains("Long description"));
    }

    #[test]
    fn thirty_attributes_fit_within_budget() {
        let mut map = serde_json::Map::new();
        map.insert("id".into(), json!("urn:ngsi-ld:Test:1"));
        map.insert("type".into(), json!("Test"));
        for i in 0usize..30usize {
            map.insert(
                format!("attr_{i:02}"),
                json!({
                    "type": "Property",
                    "value": i.saturating_mul(10)
                }),
            );
        }
        let rows = vec![Value::Object(map); 5];
        let basis = Basis {
            loaded: 5,
            total: Some(5),
            reason: BasisReason::All,
            skipped: 0,
        };
        let p = profile(&rows, Some(5), basis, None);
        assert_eq!(p.attributes.len(), 12);
        assert_eq!(p.other_attributes.len(), 18);
        assert_eq!(at(&p.to_json(), "/otherAttributes/0"), &json!("attr_12"));

        let rendered = render(&p);
        assert!(rendered.len() <= 6000);
        assert!(rendered.contains("otherAttributes:"));
    }

    #[test]
    fn empty_rows_renders_without_panicking_or_nan() {
        let basis = Basis {
            loaded: 0,
            total: Some(0),
            reason: BasisReason::All,
            skipped: 0,
        };
        let p = profile(&[], Some(0), basis, None);
        let rendered = render(&p);
        assert!(rendered.contains("count=0"));
        assert!(!rendered.contains("NaN"));
        assert!(!rendered.contains("null"));
    }

    #[test]
    fn rows_that_exist_but_did_not_load_are_never_rendered_as_none() {
        let basis = Basis {
            loaded: 0,
            total: Some(40),
            reason: BasisReason::MemoryLimit,
            skipped: 3,
        };
        let mut p = profile(&[], Some(40), basis, None);
        p.entity_type = "Bridge".into();
        let text = render(&p);
        assert!(text.contains("count=40"), "{text}");
        assert!(!text.contains("count=0"), "{text}");
        assert!(!text.contains("no entities found"), "{text}");
    }

    fn basis(loaded: usize, reason: BasisReason) -> Basis {
        Basis {
            loaded,
            total: None,
            reason,
            skipped: 0,
        }
    }

    /// One entity of `attrs`, each `{"type": "Property", "value": v}` unless already an object.
    fn entity(i: usize, attrs: Vec<(&str, Value)>) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("id".into(), json!(format!("urn:ngsi-ld:T:{i}")));
        map.insert("type".into(), json!("T"));
        for (k, v) in attrs {
            map.insert(k.into(), v);
        }
        Value::Object(map)
    }

    /// Rows with one attribute of every kind, and the shapes each kind must survive.
    fn every_kind() -> Vec<Value> {
        let long = "Most·Slovenského·národného·povstania ".repeat(4);
        (0u32..12)
            .map(|i| {
                let fi = f64::from(i);
                let first = i == 0;
                entity(usize::try_from(i).unwrap_or(0), vec![
                    ("name", json!({"type": "LanguageProperty", "languageMap": {"sk": format!("{long}{i}"), "en": format!("Place {i}")}})),
                    // decimals, a missing value, an outlier on each side, a date that is none
                    ("level", if first {
                        json!({"type": "Property", "value": null, "observedAt": "2026-09-30"})
                    } else if i == 1 {
                        json!({"type": "Property", "value": -900.0, "observedAt": "soon"})
                    } else {
                        json!({"type": "Property", "value": if i == 11 { 950.5 } else { 0.05 + fi * 0.1 }, "unitCode": "", "observedAt": format!("2026-10-{:02}T08:00:00Z", i.saturating_add(1))})
                    }),
                    ("open", json!({"type": "Property", "value": i % 3 == 0})),
                    ("built", if i % 2 == 0 {
                        json!({"type": "Property", "value": {"@type": "DateTime", "@value": format!("19{:02}-05-01T00:00:00Z", i.saturating_add(50))}})
                    } else {
                        json!({"type": "Property", "value": format!("19{:02}-05-01", i.saturating_add(50))})
                    }),
                    // six kinds: the top five and the rest as other
                    ("kind", json!({"type": "Property", "value": char::from(b'a'.saturating_add(u8::try_from(i % 6).unwrap_or(0))).to_string()})),
                    ("area", json!({"type": "GeoProperty", "value": {"type": "Polygon", "coordinates": [[[17.0 + fi, 48.0], [17.5 + fi, 48.5], [17.0 + fi, 48.5], [17.0 + fi, 48.0]]]}})),
                    ("owner", json!({"type": "Relationship", "object": format!("urn:ngsi-ld:Org:{}", i % 2)})),
                    ("address", json!({"type": "Property", "value": {"streetAddress": format!("Ulica {i}")}})),
                    ("note", json!({"type": "Property", "value": if first { "not-a-date".to_owned() } else { format!("free text {i}") }})),
                ])
            })
            .collect()
    }

    #[test]
    fn every_kind_is_profiled_and_rendered() {
        let rows = every_kind();
        let p = profile(
            &rows,
            None,
            basis(rows.len(), BasisReason::RowLimit),
            Some("en"),
        );
        assert_eq!(
            p.observed_at,
            Some(("2026-09-30".into(), "2026-10-12".into()))
        );
        // the English name, from the language map
        assert_eq!(
            p.samples.first().and_then(|s| s.name.as_deref()),
            Some("Place 0")
        );

        let json = p.to_json();
        assert_eq!(at(&json, "/basis/reason"), &json!("RowLimit"));
        assert_eq!(at(&json, "/count"), &Value::Null);
        assert_eq!(at(&json, "/observedAt/min"), &json!("2026-09-30"));
        let kinds: Vec<(&str, &Value)> = [
            "level", "open", "built", "kind", "area", "owner", "address", "note",
        ]
        .into_iter()
        .map(|a| (a, at(&json, &format!("/attributes/{a}/kind"))))
        .collect();
        assert_eq!(
            kinds,
            [
                ("level", &json!("number")),
                ("open", &json!("boolean")),
                ("built", &json!("dateTime")),
                ("kind", &json!("category")),
                ("area", &json!("geo")),
                ("owner", &json!("relationship")),
                ("address", &json!("structured")),
                ("note", &json!("text")),
            ]
        );
        let level = at(&json, "/attributes/level");
        assert_eq!(
            (
                at(level, "/n"),
                at(level, "/nulls"),
                at(level, "/outliers"),
                at(level, "/unitCode"),
                at(level, "/top3")
            ),
            (&json!(11), &json!(1), &json!(2), &Value::Null, &Value::Null)
        );
        assert_eq!(at(&json, "/attributes/open/n"), &json!(12));
        assert_eq!(at(&json, "/attributes/built/min"), &json!("1950-05-01"));
        assert_eq!(
            (
                at(&json, "/attributes/kind/distinct"),
                at(&json, "/attributes/kind/other")
            ),
            (&json!(6), &json!(2))
        );
        assert_eq!(
            at(&json, "/attributes/area/bbox"),
            &json!([17.0, 48.0, 28.5, 48.5])
        );
        assert_eq!(at(&json, "/attributes/owner/distinct"), &json!(2));
        assert_eq!(at(&json, "/attributes/address/n"), &json!(12));
        assert_eq!(at(&json, "/attributes/note/n"), &json!(12));
        assert_eq!(at(&json, "/samples/0/name"), &json!("Place 0"));

        let text = render(&p);
        for part in [
            "count=12 basis=12 of unknown (first by id)",
            "observedAt: min=2026-09-30 max=2026-10-12",
            "nulls=1",
            "outliers=2",
            "other=2",
            "name=\"Place 0\"",
        ] {
            assert!(text.contains(part), "{part}: {text}");
        }
        assert!(!text.contains("free text"), "{text}");
    }

    #[test]
    fn a_name_without_the_asked_language_is_its_first_and_is_cut_at_80() {
        let rows = every_kind();
        // `en` sorts before `sk`: the first entry of the map
        let p = profile(&rows, Some(12), basis(12, BasisReason::All), Some("hu"));
        assert_eq!(
            p.samples.first().and_then(|s| s.name.as_deref()),
            Some("Place 0")
        );
        let p = profile(&rows, Some(12), basis(12, BasisReason::All), None);
        assert_eq!(
            p.samples.first().and_then(|s| s.name.as_deref()),
            Some("Place 0")
        );
        let named = |name: Value| {
            let rows = [entity(1, vec![("name", name)])];
            profile(&rows, Some(1), basis(1, BasisReason::All), None)
                .samples
                .first()
                .and_then(|s| s.name.clone())
        };
        assert_eq!(
            named(json!({"type": "Property", "value": "x".repeat(100)})).map(|n| n.chars().count()),
            Some(80)
        );
        assert_eq!(named(json!({"type": "LanguageProperty"})), None);
        assert_eq!(
            named(json!({"type": "LanguageProperty", "languageMap": {}})),
            None
        );
        assert_eq!(
            named(json!({"type": "LanguageProperty", "languageMap": {"sk": 1}})),
            None
        );
    }

    #[test]
    fn a_sample_value_is_its_target_text_number_flag_or_json_cut_at_80() {
        let long = "y".repeat(100);
        let cases = [
            (
                json!({"type": "Relationship", "object": "urn:ngsi-ld:Org:1"}),
                "urn:ngsi-ld:Org:1".to_owned(),
            ),
            (
                json!({"type": "Relationship"}),
                "{\"type\":\"Relationship\"}".to_owned(),
            ),
            (json!({"value": long}), "y".repeat(80)),
            (json!({"value": "short"}), "short".to_owned()),
            (json!({"value": 2.5}), "2.50".to_owned()),
            (json!({"value": true}), "true".to_owned()),
            (json!({"value": [1, 2]}), "{\"value\":[1,2]}".to_owned()),
            (json!([long]), format!("[\"{}", "y".repeat(78))),
        ];
        for (node, want) in cases {
            assert_eq!(format_sample_value(&node, None), want, "{node}");
        }
    }

    #[test]
    fn rows_that_are_no_objects_and_values_of_no_kind_are_skipped() {
        let rows = vec![
            json!("not an entity"),
            entity(
                1,
                vec![
                    ("a", json!({"type": "Property", "value": 1})),
                    ("b", json!({"type": "Property", "value": "x"})),
                ],
            ),
            entity(
                2,
                vec![
                    ("a", json!({"type": "Property", "value": "one"})),
                    ("b", json!({"type": "Property", "value": "y"})),
                ],
            ),
            entity(3, vec![("a", json!("bare"))]),
        ];
        let p = profile(&rows, Some(4), basis(4, BasisReason::All), None);
        assert_eq!(p.entity_type, "");
        assert_eq!(p.samples.len(), 3);
        // entity 3 has no `b`: its sample leaves it out
        assert_eq!(p.samples.last().map(|s| s.values.len()), Some(1));
        // a sampled attribute an entity lacks is left out of its sample
        let few = [
            entity(1, vec![("a", json!({"type": "Property", "value": 1}))]),
            entity(2, vec![("a", json!({"type": "Property", "value": 2}))]),
            entity(3, vec![("z", json!({"type": "Property", "value": "x"}))]),
        ];
        let lacking = profile(&few, Some(3), basis(3, BasisReason::All), None);
        let sampled: Vec<usize> = lacking.samples.iter().map(|s| s.values.len()).collect();
        assert_eq!(sampled, [1, 1, 0]);
        let json = p.to_json();
        assert_eq!(
            (
                at(&json, "/attributes/a/kind"),
                at(&json, "/attributes/b/kind")
            ),
            (&json!("number"), &json!("text"))
        );
        // a geometry with too few numbers or none, a relationship without an object
        let geo = analyze_geo(
            "g",
            &[
                &json!({"type": "GeoProperty", "value": {"coordinates": [["a"], [1.0, "b"]]}}),
                &json!({"type": "GeoProperty"}),
            ],
        );
        assert_eq!(
            geo.map(|g| attr_to_json(&g)),
            Some(
                json!({"kind": "geo", "n": 2, "bbox": [0.0, 0.0, 0.0, 0.0], "centroid": [0.0, 0.0]})
            )
        );
        let rel = analyze_relationship("r", &[&json!({"type": "Relationship"})]);
        assert_eq!(
            rel.map(|r| attr_to_json(&r)),
            Some(json!({"kind": "relationship", "n": 1, "distinct": 0, "top3": []}))
        );
        let text = AttrStat::Text {
            name: "t".into(),
            n: 1,
            share_present: 1.0,
        };
        assert_eq!(number_to_json(&text), Value::Null);
        assert_eq!(render_number_stat(&text), "");
    }

    #[test]
    fn a_kind_needs_half_of_the_values() {
        let v = |x: Value| json!({"type": "Property", "value": x});
        let half = |a: Value, b: Value, c: Value| [v(a), v(b), v(c), v(json!("z"))];
        let kind_of = |values: &[Value]| {
            let refs: Vec<&Value> = values.iter().collect();
            at(
                &attr_to_json(&analyze_attribute("a", &refs, values.len(), None)),
                "/kind",
            )
            .clone()
        };
        // one number, one flag, one date in four: none of those kinds
        assert_eq!(
            kind_of(&half(json!(1), json!("p"), json!("q"))),
            json!("text")
        );
        assert_eq!(
            kind_of(&half(json!(true), json!("p"), json!("q"))),
            json!("text")
        );
        assert_eq!(
            kind_of(&half(json!("2026-01-01"), json!("p"), json!("q"))),
            json!("text")
        );
        // a date object without its value, a bare value: no date
        let odd = [
            json!({"type": "Property", "value": {"@type": "DateTime"}}),
            json!("bare"),
        ];
        assert_eq!(kind_of(&odd), json!("structured"));
        // one string in four is no category either
        assert_eq!(
            kind_of(&[v(json!("p")), v(json!([1])), v(json!([2])), v(json!([3]))]),
            json!("structured")
        );
        // more than 20 kinds is text, however often each recurs
        let many: Vec<Value> = (0..44).map(|i| v(json!(format!("k{}", i % 22)))).collect();
        assert_eq!(kind_of(&many), json!("text"));
        // an integer past i64 still counts, it just has no top value
        let huge = [v(json!(1e20)), v(json!(1e20))];
        let refs: Vec<&Value> = huge.iter().collect();
        let stat = attr_to_json(&analyze_attribute("a", &refs, 2, None));
        assert_eq!(at(&stat, "/top3"), &json!([]));
    }

    #[test]
    fn one_value_has_no_spread_and_one_bucket() {
        let rows = vec![entity(
            1,
            vec![(
                "a",
                json!({"type": "Property", "value": 7, "unitCode": "MTR"}),
            )],
        )];
        let p = profile(&rows, Some(1), basis(1, BasisReason::All), None);
        let a = at(&p.to_json(), "/attributes/a").clone();
        assert_eq!(
            (
                at(&a, "/stddev"),
                at(&a, "/hist"),
                at(&a, "/top3"),
                at(&a, "/unitCode")
            ),
            (
                &json!(0.0),
                &json!([{"start": 7.0, "end": 7.0, "count": 1}]),
                &json!([{"value": 7, "count": 1}]),
                &json!("MTR")
            )
        );
        assert!(percentile(&[], 0.5).abs() < f64::EPSILON);
        assert!((percentile(&[1.0, 3.0], 1.0) - 3.0).abs() < f64::EPSILON);
        assert!((percentile(&[1.0, 3.0], 0.5) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_number_reads_with_three_significant_figures() {
        let cases = [
            (f64::NAN, "0"),
            (f64::INFINITY, "0"),
            (12.0, "12"),
            (123.4, "123"),
            (12.34, "12.3"),
            (1.234, "1.23"),
            (0.1234, "0.123"),
            (0.01234, "0.0123"),
        ];
        for (v, want) in cases {
            assert_eq!(fmt_num(v), want, "{v}");
        }
    }

    #[test]
    fn the_basis_says_why_loading_stopped() {
        let mut b = Basis {
            loaded: 3,
            total: Some(9),
            reason: BasisReason::All,
            skipped: 2,
        };
        let says = |b: &Basis| (render_basis(b), b.reason.as_str());
        assert_eq!(says(&b), ("3 of 9 (all), 2 skipped (>1MiB)".into(), "All"));
        b.reason = BasisReason::MemoryLimit;
        assert_eq!(
            says(&b),
            (
                "3 of 9 (stopped by memory limit), 2 skipped (>1MiB)".into(),
                "MemoryLimit"
            )
        );
        b.reason = BasisReason::Busy;
        assert_eq!(
            says(&b),
            ("3 of 9 (server busy), 2 skipped (>1MiB)".into(), "Busy")
        );
        b.reason = BasisReason::Tight;
        assert_eq!(
            says(&b),
            ("summary unavailable now, server busy".into(), "Tight")
        );
    }

    #[test]
    fn a_tight_server_answers_the_count_alone() {
        let rows = every_kind();
        let p = profile(&rows, Some(12), basis(0, BasisReason::Tight), None);
        assert_eq!(p.attributes, Vec::new());
        let json = p.to_json();
        assert_eq!(
            at(&json, "/unavailable"),
            &json!("summary unavailable now, server busy")
        );
        assert_eq!(at(&json, "/attributes"), &Value::Null);
        let text = render(&p);
        assert!(
            text.contains("count=12 basis=summary unavailable now"),
            "{text}"
        );
        assert_eq!(text.lines().count(), 1, "{text}");
        // no count from the broker: the loaded rows count
        let p = profile(&[], None, basis(0, BasisReason::Tight), None);
        assert!(render(&p).contains("count=0"));
    }

    #[test]
    fn a_profile_without_samples_renders_none() {
        let rows = every_kind();
        let mut p = profile(&rows, Some(12), basis(12, BasisReason::All), None);
        p.samples.clear();
        assert!(!render(&p).contains("sampleEntities"));
    }

    #[test]
    fn an_answer_past_6000_characters_drops_samples_then_statistics_then_lines() {
        let wide = |name_len: usize, attrs: usize| -> Vec<Value> {
            let names: Vec<String> = (0..attrs)
                .map(|a| format!("{a:02}{}", "n".repeat(name_len)))
                .collect();
            (0..5)
                .map(|i| {
                    entity(
                        i,
                        names
                            .iter()
                            .map(|n| {
                                (
                                    n.as_str(),
                                    json!({"type": "Property", "value": "v".repeat(70)}),
                                )
                            })
                            .collect(),
                    )
                })
                .collect()
        };
        let shape = |name_len: usize, attrs: usize| {
            let rows = wide(name_len, attrs);
            let text = render(&profile(&rows, Some(5), basis(5, BasisReason::All), None));
            (
                text.len() <= 6000,
                text.contains("sampleEntities"),
                text.contains("otherAttributes: "),
                text.lines().count(),
            )
        };
        // samples alone overflow: they go, every statistic stays
        assert_eq!(shape(300, 5), (true, false, false, 6));
        // the statistics overflow too: the last ones are only named
        let (fits, _, named, _) = shape(450, 12);
        assert_eq!((fits, named), (true, true));
        // even the names overflow: whole lines are cut
        assert_eq!(shape(1000, 12), (true, false, false, 1));
    }
}
