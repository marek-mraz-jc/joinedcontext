//! Prometheus exporter with Bento 1.22's metric names, label sets and text format
//! (`internal/impl/prometheus/metrics_prometheus.go`, `internal/component/metrics/namespaced.go`,
//! `internal/component/metrics/mapping.go`).
//!
//! - Counters, gauges and timers are families of series keyed by their sorted label pairs, as
//!   `client_golang` prints them (`name{a="1",b="2"} 21`, families sorted by name).
//! - Timers are summaries (quantiles over a bounded reservoir of the latest observations, `_sum`,
//!   `_count`) or, with `use_histogram_timing`, histograms whose observations are converted from
//!   nanoseconds into seconds (`_bucket{le=...}`, `_sum`, `_count`), as Bento does.
//! - `metrics.mapping` (Bloblang over the metric name with the labels as metadata) renames a
//!   series, adds or rewrites labels, or drops it with `deleted()`; it runs once per series, when
//!   the series is first asked for.
//! - A deleted stream's counters and gauges live on and continue when the same id is created
//!   again (Bento's registry keeps them until a restart); its timings are dropped at once. The
//!   number of series is capped (`max_series`, default 20,000): past it the series of the
//!   longest-deleted stream go first, and a new series is refused (counted nowhere) while no
//!   deleted stream is left to evict.
//! - Label values are escaped per the exposition format (`\\`, `\"`, `\n`), so a stream id or a
//!   label holding a quote or a newline can neither break a line nor forge a series; a metric or
//!   label name outside `[a-zA-Z_:][a-zA-Z0-9_:]*` is ignored, as Bento ignores it.
//! - Every lock recovers from poisoning: a panicking thread never takes `/metrics` down.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use ekiben_bloblang::value::format_go_float;
use ekiben_bloblang::{Environment, Executor, SimplePartView, Value};
use serde_json::Value as JsonValue;

use crate::error::CoreError;

/// Prometheus' `DefBuckets` (seconds), Bento's default when `histogram_buckets` is empty.
pub const DEFAULT_HISTOGRAM_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];
/// Bento's default summary objectives (quantiles; the error margins do not apply to a reservoir).
pub const DEFAULT_SUMMARY_QUANTILES: [f64; 3] = [0.5, 0.9, 0.99];
/// Default cap on the number of series the exporter holds.
pub const DEFAULT_MAX_SERIES: usize = 20_000;
/// Observations a summary series keeps for its quantiles (the latest ones).
pub const SUMMARY_RESERVOIR_SIZE: usize = 1024;

const HELP_COUNTER: &str = "Bento Counter metric";
const HELP_GAUGE: &str = "Bento Gauge metric";
const HELP_TIMER: &str = "Bento Timing metric";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(l: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(PoisonError::into_inner)
}

/// The `metrics:` section of a config (`metrics: {prometheus: {...}, mapping: ...}`) and the
/// runner's `--set metrics.<field>=<value>` arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricsConfig {
    pub use_histogram_timing: bool,
    /// Bucket upper bounds in seconds; empty means [`DEFAULT_HISTOGRAM_BUCKETS`].
    pub histogram_buckets: Vec<f64>,
    pub summary_quantiles: Vec<f64>,
    pub add_process_metrics: bool,
    /// Bloblang mapping over each metric name (`metrics.mapping`); empty means none.
    pub mapping: String,
    /// ekiben extension: the most series held at once.
    pub max_series: usize,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            use_histogram_timing: false,
            histogram_buckets: Vec::new(),
            summary_quantiles: DEFAULT_SUMMARY_QUANTILES.to_vec(),
            add_process_metrics: false,
            mapping: String::new(),
            max_series: DEFAULT_MAX_SERIES,
        }
    }
}

fn bool_field(
    map: &serde_json::Map<String, JsonValue>,
    key: &str,
) -> Result<Option<bool>, CoreError> {
    match map.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Bool(b)) => Ok(Some(*b)),
        Some(JsonValue::String(s)) if s == "true" || s == "false" => Ok(Some(s == "true")),
        Some(other) => Err(CoreError::config(format!(
            "metrics.prometheus.{key}: expected a boolean, got {other}"
        ))),
    }
}

impl MetricsConfig {
    /// Reads a `metrics:` section as Bento writes it. Only the `prometheus` exporter exists in
    /// ekiben; its push gateway and file output are refused (a push is an outbound connection
    /// outside the outbound rule; a file is outside the file root).
    pub fn from_json(section: &JsonValue) -> Result<Self, CoreError> {
        let mut conf = Self::default();
        let Some(map) = section.as_object() else {
            if section.is_null() {
                return Ok(conf);
            }
            return Err(CoreError::config("metrics: expected an object"));
        };
        for key in map.keys() {
            match key.as_str() {
                "prometheus" | "mapping" | "max_series" => {}
                "type" => {
                    if map.get("type").and_then(JsonValue::as_str) != Some("prometheus") {
                        return Err(CoreError::config(
                            "metrics: only the prometheus exporter is built into ekiben",
                        ));
                    }
                }
                other => {
                    return Err(CoreError::config(format!(
                        "metrics: exporter '{other}' is not built into ekiben (only prometheus)"
                    )))
                }
            }
        }
        if let Some(m) = map.get("mapping") {
            conf.mapping = m
                .as_str()
                .ok_or_else(|| CoreError::config("metrics.mapping: expected a string"))?
                .to_string();
        }
        if let Some(n) = map.get("max_series") {
            let n = n
                .as_u64()
                .or_else(|| n.as_str().and_then(|s| s.parse().ok()))
                .ok_or_else(|| CoreError::config("metrics.max_series: expected a number"))?;
            if n == 0 {
                return Err(CoreError::config("metrics.max_series must be at least 1"));
            }
            conf.max_series = usize::try_from(n).unwrap_or(usize::MAX);
        }
        let prom = match map.get("prometheus") {
            None | Some(JsonValue::Null) => return Ok(conf),
            Some(JsonValue::Object(p)) => p,
            Some(_) => return Err(CoreError::config("metrics.prometheus: expected an object")),
        };
        for (key, val) in prom {
            match key.as_str() {
                "use_histogram_timing"
                | "add_process_metrics"
                | "add_go_metrics"
                | "use_default_registry"
                | "histogram_buckets"
                | "summary_quantiles_objectives"
                | "push_job_name" => {}
                "push_url" | "push_interval" | "push_basic_auth" | "file_output_path" => {
                    let empty = match val {
                        JsonValue::Null => true,
                        JsonValue::String(s) => s.is_empty(),
                        JsonValue::Object(o) => {
                            o.values().all(|v| v.as_str().is_none_or(str::is_empty))
                        }
                        _ => false,
                    };
                    if !empty {
                        return Err(CoreError::config(format!(
                            "metrics.prometheus.{key} is not supported by ekiben: /metrics is scraped, never pushed or written"
                        )));
                    }
                }
                other => {
                    return Err(CoreError::config(format!(
                        "metrics.prometheus: field {other} is not recognised"
                    )))
                }
            }
        }
        if let Some(b) = bool_field(prom, "use_histogram_timing")? {
            conf.use_histogram_timing = b;
        }
        if let Some(b) = bool_field(prom, "add_process_metrics")? {
            conf.add_process_metrics = b;
        }
        if let Some(arr) = prom.get("histogram_buckets") {
            let list = arr.as_array().ok_or_else(|| {
                CoreError::config(
                    "metrics.prometheus.histogram_buckets: expected a list of numbers",
                )
            })?;
            let mut buckets = Vec::with_capacity(list.len());
            for v in list {
                let b = v
                    .as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                    .filter(|b: &f64| b.is_finite())
                    .ok_or_else(|| {
                        CoreError::config(format!(
                            "metrics.prometheus.histogram_buckets: '{v}' is not a finite number"
                        ))
                    })?;
                buckets.push(b);
            }
            buckets.sort_by(f64::total_cmp);
            buckets.dedup();
            conf.histogram_buckets = buckets;
        }
        if let Some(arr) = prom
            .get("summary_quantiles_objectives")
            .and_then(JsonValue::as_array)
        {
            let mut qs = Vec::with_capacity(arr.len());
            for obj in arr {
                let q = obj
                    .get("quantile")
                    .and_then(JsonValue::as_f64)
                    .unwrap_or(0.0);
                if !(0.0..=1.0).contains(&q) {
                    return Err(CoreError::config(format!(
                        "metrics.prometheus.summary_quantiles_objectives: quantile {q} is outside 0..1"
                    )));
                }
                qs.push(q);
            }
            if !qs.is_empty() {
                qs.sort_by(f64::total_cmp);
                qs.dedup();
                conf.summary_quantiles = qs;
            }
        }
        Ok(conf)
    }

    fn buckets(&self) -> Vec<f64> {
        if self.histogram_buckets.is_empty() {
            DEFAULT_HISTOGRAM_BUCKETS.to_vec()
        } else {
            self.histogram_buckets.clone()
        }
    }
}

/// A compiled `metrics.mapping` (Bento's `metrics.Mapping`).
pub struct MetricMapping {
    exec: Option<Executor>,
}

impl MetricMapping {
    /// Compiles `mapping`; an empty mapping changes nothing. The mapping runs without host
    /// environment access.
    pub fn new(mapping: &str) -> Result<Self, CoreError> {
        if mapping.trim().is_empty() {
            return Ok(Self { exec: None });
        }
        let exec = Environment::new()
            .compile_mapping(mapping)
            .map_err(|e| CoreError::config(format!("metrics.mapping: {e}")))?;
        Ok(Self { exec: Some(exec) })
    }

    pub fn is_empty(&self) -> bool {
        self.exec.is_none()
    }

    /// Bento's `mapPath`: returns the new name (empty when the series is deleted) and the label
    /// names (sorted) and values. A failing mapping keeps the name and drops the labels; a result
    /// that is not a string keeps everything as it was.
    pub fn map_path(
        &self,
        path: &str,
        names: &[String],
        values: &[String],
    ) -> (String, Vec<String>, Vec<String>) {
        let Some(exec) = &self.exec else {
            return (path.to_string(), names.to_vec(), values.to_vec());
        };
        let meta: BTreeMap<String, Value> = names
            .iter()
            .zip(values.iter())
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        let parts = [SimplePartView {
            raw: path.as_bytes().to_vec(),
            structured: Some(Value::String(path.to_string())),
            meta: meta.clone(),
            error: None,
        }];
        let mut out_meta = meta;
        match exec.map_part(0, &parts, &mut out_meta, None) {
            Err(_) => (path.to_string(), Vec::new(), Vec::new()),
            Ok(None) => (String::new(), Vec::new(), Vec::new()),
            Ok(Some(result)) => {
                let out_names: Vec<String> = out_meta.keys().cloned().collect();
                let out_values: Vec<String> =
                    out_meta.values().map(Value::to_string_coerced).collect();
                match result {
                    Value::Nothing => (path.to_string(), out_names, out_values),
                    Value::String(s) => (s, out_names, out_values),
                    _ => (path.to_string(), names.to_vec(), values.to_vec()),
                }
            }
        }
    }
}

/// The stream (and organization) a series belongs to: what `/metrics` filters an organization's
/// view by and what a stream's deletion marks.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SeriesOwner {
    pub org: String,
    pub stream: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FamilyKind {
    Counter,
    Gauge,
    Timer,
}

struct AtomicF64(AtomicU64);

impl AtomicF64 {
    fn new(v: f64) -> Self {
        Self(AtomicU64::new(v.to_bits()))
    }

    fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn set(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }

    fn add(&self, delta: f64) {
        let mut cur = self.0.load(Ordering::Relaxed);
        loop {
            let next = (f64::from_bits(cur) + delta).to_bits();
            match self
                .0
                .compare_exchange_weak(cur, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(actual) => cur = actual,
            }
        }
    }
}

struct Reservoir {
    samples: Vec<f64>,
    cursor: usize,
}

impl Reservoir {
    fn record(&mut self, v: f64) {
        if self.samples.len() < SUMMARY_RESERVOIR_SIZE {
            self.samples.push(v);
        } else if let Some(slot) = self.samples.get_mut(self.cursor % SUMMARY_RESERVOIR_SIZE) {
            *slot = v;
        }
        self.cursor = self.cursor.wrapping_add(1);
    }

    fn quantile(&self, q: f64) -> f64 {
        if self.samples.is_empty() {
            return f64::NAN;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_by(f64::total_cmp);
        let last = sorted.len().saturating_sub(1);
        let idx = ((last as f64) * q).round() as usize;
        sorted.get(idx.min(last)).copied().unwrap_or(f64::NAN)
    }
}

enum SeriesValue {
    Scalar(AtomicF64),
    Summary {
        sum: AtomicF64,
        count: AtomicU64,
        samples: Mutex<Reservoir>,
    },
    Histogram {
        sum: AtomicF64,
        count: AtomicU64,
        bounds: Arc<Vec<f64>>,
        counts: Vec<AtomicU64>,
    },
}

/// The live value of one series; handles hold it so the hot path takes no registry lock.
pub struct SeriesData {
    value: SeriesValue,
}

impl SeriesData {
    fn scalar() -> Self {
        Self {
            value: SeriesValue::Scalar(AtomicF64::new(0.0)),
        }
    }

    fn summary() -> Self {
        Self {
            value: SeriesValue::Summary {
                sum: AtomicF64::new(0.0),
                count: AtomicU64::new(0),
                samples: Mutex::new(Reservoir {
                    samples: Vec::new(),
                    cursor: 0,
                }),
            },
        }
    }

    fn histogram(bounds: Arc<Vec<f64>>) -> Self {
        let counts = (0..bounds.len()).map(|_| AtomicU64::new(0)).collect();
        Self {
            value: SeriesValue::Histogram {
                sum: AtomicF64::new(0.0),
                count: AtomicU64::new(0),
                bounds,
                counts,
            },
        }
    }

    fn add(&self, delta: f64) {
        if let SeriesValue::Scalar(v) = &self.value {
            v.add(delta);
        }
    }

    fn set(&self, value: f64) {
        if let SeriesValue::Scalar(v) = &self.value {
            v.set(value);
        }
    }

    /// The scalar value of a counter or gauge (0 for a timer).
    pub fn value(&self) -> f64 {
        match &self.value {
            SeriesValue::Scalar(v) => v.get(),
            _ => 0.0,
        }
    }

    fn observe_ns(&self, ns: u64) {
        match &self.value {
            SeriesValue::Scalar(_) => {}
            SeriesValue::Summary {
                sum,
                count,
                samples,
            } => {
                let v = ns as f64;
                sum.add(v);
                count.fetch_add(1, Ordering::Relaxed);
                lock(samples).record(v);
            }
            SeriesValue::Histogram {
                sum,
                count,
                bounds,
                counts,
            } => {
                let secs = ns as f64 / 1_000_000_000.0;
                sum.add(secs);
                count.fetch_add(1, Ordering::Relaxed);
                if let Some(i) = bounds.iter().position(|b| secs <= *b) {
                    if let Some(c) = counts.get(i) {
                        c.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}

/// A counter handle; a deleted, refused or invalid series is a no-op handle.
#[derive(Clone, Default)]
pub struct Counter(Option<Arc<SeriesData>>);

impl Counter {
    pub fn noop() -> Self {
        Self(None)
    }

    pub fn incr(&self, count: u64) {
        if let Some(s) = &self.0 {
            s.add(count as f64);
        }
    }

    pub fn incr_f64(&self, count: f64) {
        if let Some(s) = &self.0 {
            if count.is_finite() && count >= 0.0 {
                s.add(count);
            }
        }
    }

    /// The current value (0 for a no-op handle).
    pub fn value(&self) -> f64 {
        self.0.as_ref().map_or(0.0, |s| s.value())
    }

    pub fn is_noop(&self) -> bool {
        self.0.is_none()
    }
}

/// A gauge handle (`buffer_backlog` and the like).
#[derive(Clone, Default)]
pub struct Gauge(Option<Arc<SeriesData>>);

impl Gauge {
    pub fn noop() -> Self {
        Self(None)
    }

    pub fn set(&self, value: i64) {
        if let Some(s) = &self.0 {
            s.set(value as f64);
        }
    }

    pub fn set_f64(&self, value: f64) {
        if let Some(s) = &self.0 {
            s.set(value);
        }
    }

    pub fn incr(&self, count: i64) {
        if let Some(s) = &self.0 {
            s.add(count as f64);
        }
    }

    pub fn decr(&self, count: i64) {
        if let Some(s) = &self.0 {
            s.add(-(count as f64));
        }
    }

    pub fn value(&self) -> f64 {
        self.0.as_ref().map_or(0.0, |s| s.value())
    }
}

/// A timer handle; `timing` takes nanoseconds.
#[derive(Clone, Default)]
pub struct Timer(Option<Arc<SeriesData>>);

impl Timer {
    pub fn noop() -> Self {
        Self(None)
    }

    pub fn timing(&self, ns: u64) {
        if let Some(s) = &self.0 {
            s.observe_ns(ns);
        }
    }
}

struct SeriesEntry {
    data: Arc<SeriesData>,
    owner: Option<SeriesOwner>,
}

struct Family {
    kind: FamilyKind,
    series: BTreeMap<Vec<(String, String)>, SeriesEntry>,
}

#[derive(Default)]
struct DeletedStreams {
    seq: u64,
    by_owner: HashMap<SeriesOwner, u64>,
}

fn is_valid_metric_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == ':' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

fn is_valid_label_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Escapes a label value per the text exposition format: `\` as `\\`, `"` as `\"`, newline as `\n`.
pub fn escape_label_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

fn write_labels(out: &mut String, labels: &[(String, String)], extra: Option<(&str, &str)>) {
    if labels.is_empty() && extra.is_none() {
        return;
    }
    out.push('{');
    let mut first = true;
    for (k, v) in labels {
        if !first {
            out.push(',');
        }
        first = false;
        let _ = write!(out, "{k}=\"{}\"", escape_label_value(v));
    }
    if let Some((k, v)) = extra {
        if !first {
            out.push(',');
        }
        let _ = write!(out, "{k}=\"{}\"", escape_label_value(v));
    }
    out.push('}');
}

/// The process-wide registry of series and the `/metrics` renderer.
pub struct PromExporter {
    config: MetricsConfig,
    buckets: Arc<Vec<f64>>,
    mapping: Arc<MetricMapping>,
    families: RwLock<BTreeMap<String, Family>>,
    total: AtomicUsize,
    deleted: Mutex<DeletedStreams>,
}

impl PromExporter {
    /// Builds an exporter; fails when `metrics.mapping` does not parse.
    pub fn new(config: MetricsConfig) -> Result<Self, CoreError> {
        let mapping = Arc::new(MetricMapping::new(&config.mapping)?);
        let buckets = Arc::new(config.buckets());
        Ok(Self {
            config,
            buckets,
            mapping,
            families: RwLock::new(BTreeMap::new()),
            total: AtomicUsize::new(0),
            deleted: Mutex::new(DeletedStreams::default()),
        })
    }

    /// An exporter with Bento's defaults (summary timings, no mapping).
    pub fn with_defaults() -> Self {
        Self {
            config: MetricsConfig::default(),
            buckets: Arc::new(DEFAULT_HISTOGRAM_BUCKETS.to_vec()),
            mapping: Arc::new(MetricMapping { exec: None }),
            families: RwLock::new(BTreeMap::new()),
            total: AtomicUsize::new(0),
            deleted: Mutex::new(DeletedStreams::default()),
        }
    }

    pub fn config(&self) -> &MetricsConfig {
        &self.config
    }

    /// The configured `metrics.mapping`.
    pub fn mapping(&self) -> Arc<MetricMapping> {
        self.mapping.clone()
    }

    /// How many series the exporter holds.
    pub fn series_count(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    fn get_or_create(
        &self,
        name: &str,
        kind: FamilyKind,
        mut labels: Vec<(String, String)>,
        owner: Option<&SeriesOwner>,
    ) -> Option<Arc<SeriesData>> {
        if !is_valid_metric_name(name) || labels.iter().any(|(k, _)| !is_valid_label_name(k)) {
            return None;
        }
        labels.sort();
        labels.dedup_by(|a, b| a.0 == b.0);
        {
            let families = read(&self.families);
            if let Some(fam) = families.get(name) {
                if fam.kind != kind {
                    return None;
                }
                if let Some(entry) = fam.series.get(&labels) {
                    return Some(entry.data.clone());
                }
            }
        }
        let data = Arc::new(match kind {
            FamilyKind::Counter | FamilyKind::Gauge => SeriesData::scalar(),
            FamilyKind::Timer if self.config.use_histogram_timing => {
                SeriesData::histogram(self.buckets.clone())
            }
            FamilyKind::Timer => SeriesData::summary(),
        });
        {
            let mut families = write(&self.families);
            let fam = families.entry(name.to_string()).or_insert_with(|| Family {
                kind,
                series: BTreeMap::new(),
            });
            if fam.kind != kind {
                return None;
            }
            if let Some(entry) = fam.series.get(&labels) {
                return Some(entry.data.clone());
            }
            fam.series.insert(
                labels.clone(),
                SeriesEntry {
                    data: data.clone(),
                    owner: owner.cloned(),
                },
            );
            self.total.fetch_add(1, Ordering::Relaxed);
        }
        if self.total.load(Ordering::Relaxed) > self.config.max_series {
            self.evict_deleted();
            if self.total.load(Ordering::Relaxed) > self.config.max_series {
                let mut families = write(&self.families);
                if let Some(fam) = families.get_mut(name) {
                    if fam.series.remove(&labels).is_some() {
                        self.total.fetch_sub(1, Ordering::Relaxed);
                    }
                    if fam.series.is_empty() {
                        families.remove(name);
                    }
                }
                return None;
            }
        }
        Some(data)
    }

    /// Evicts the series of the longest-deleted streams until the cap holds again.
    fn evict_deleted(&self) {
        while self.total.load(Ordering::Relaxed) > self.config.max_series {
            let oldest = {
                let deleted = lock(&self.deleted);
                deleted
                    .by_owner
                    .iter()
                    .min_by_key(|(_, seq)| **seq)
                    .map(|(o, _)| o.clone())
            };
            let Some(owner) = oldest else {
                return;
            };
            lock(&self.deleted).by_owner.remove(&owner);
            self.remove_owned(|entry_owner, _| entry_owner == Some(&owner));
        }
    }

    fn remove_owned(&self, matches: impl Fn(Option<&SeriesOwner>, FamilyKind) -> bool) {
        let mut families = write(&self.families);
        let mut removed = 0usize;
        families.retain(|_, fam| {
            let kind = fam.kind;
            let before = fam.series.len();
            fam.series
                .retain(|_, entry| !matches(entry.owner.as_ref(), kind));
            removed += before - fam.series.len();
            !fam.series.is_empty()
        });
        self.total.fetch_sub(
            removed.min(self.total.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
    }

    /// Marks a stream deleted: its timings go at once, its counters and gauges stay (and continue
    /// when the id is created again) until the cap evicts them, oldest-deleted first.
    pub fn stream_deleted(&self, owner: &SeriesOwner) {
        {
            let mut deleted = lock(&self.deleted);
            deleted.seq = deleted.seq.wrapping_add(1);
            let seq = deleted.seq;
            deleted.by_owner.insert(owner.clone(), seq);
        }
        self.remove_owned(|o, kind| kind == FamilyKind::Timer && o == Some(owner));
    }

    /// Marks a stream live again (its retained counters continue).
    pub fn stream_created(&self, owner: &SeriesOwner) {
        lock(&self.deleted).by_owner.remove(owner);
    }

    /// Whether the stream's series are retained as a deleted stream's.
    pub fn is_deleted(&self, owner: &SeriesOwner) -> bool {
        lock(&self.deleted).by_owner.contains_key(owner)
    }

    /// Renders every visible series in the text exposition format (version 0.0.4). `visible`
    /// gets the series' owner (`None` for process-wide series).
    pub fn render(&self, visible: &dyn Fn(Option<&SeriesOwner>) -> bool) -> String {
        let mut out = String::new();
        {
            let families = read(&self.families);
            for (name, fam) in families.iter() {
                let rows: Vec<(&Vec<(String, String)>, &SeriesEntry)> = fam
                    .series
                    .iter()
                    .filter(|(_, e)| visible(e.owner.as_ref()))
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                let (help, ty) = match fam.kind {
                    FamilyKind::Counter => (HELP_COUNTER, "counter"),
                    FamilyKind::Gauge => (HELP_GAUGE, "gauge"),
                    FamilyKind::Timer if self.config.use_histogram_timing => {
                        (HELP_TIMER, "histogram")
                    }
                    FamilyKind::Timer => (HELP_TIMER, "summary"),
                };
                let _ = writeln!(out, "# HELP {name} {help}");
                let _ = writeln!(out, "# TYPE {name} {ty}");
                for (labels, entry) in rows {
                    self.render_series(&mut out, name, labels, &entry.data);
                }
            }
        }
        if self.config.add_process_metrics && visible(None) {
            render_process_metrics(&mut out);
        }
        out
    }

    fn render_series(
        &self,
        out: &mut String,
        name: &str,
        labels: &[(String, String)],
        data: &SeriesData,
    ) {
        match &data.value {
            SeriesValue::Scalar(v) => {
                out.push_str(name);
                write_labels(out, labels, None);
                let _ = writeln!(out, " {}", format_go_float(v.get()));
            }
            SeriesValue::Summary {
                sum,
                count,
                samples,
            } => {
                let reservoir = lock(samples);
                for q in &self.config.summary_quantiles {
                    out.push_str(name);
                    let qs = format_go_float(*q);
                    write_labels(out, labels, Some(("quantile", &qs)));
                    let _ = writeln!(out, " {}", format_go_float(reservoir.quantile(*q)));
                }
                drop(reservoir);
                let _ = write!(out, "{name}_sum");
                write_labels(out, labels, None);
                let _ = writeln!(out, " {}", format_go_float(sum.get()));
                let _ = write!(out, "{name}_count");
                write_labels(out, labels, None);
                let _ = writeln!(out, " {}", count.load(Ordering::Relaxed));
            }
            SeriesValue::Histogram {
                sum,
                count,
                bounds,
                counts,
            } => {
                let mut cumulative = 0u64;
                for (b, c) in bounds.iter().zip(counts.iter()) {
                    cumulative = cumulative.saturating_add(c.load(Ordering::Relaxed));
                    let _ = write!(out, "{name}_bucket");
                    let le = format_go_float(*b);
                    write_labels(out, labels, Some(("le", &le)));
                    let _ = writeln!(out, " {cumulative}");
                }
                let total = count.load(Ordering::Relaxed);
                let _ = write!(out, "{name}_bucket");
                write_labels(out, labels, Some(("le", "+Inf")));
                let _ = writeln!(out, " {total}");
                let _ = write!(out, "{name}_sum");
                write_labels(out, labels, None);
                let _ = writeln!(out, " {}", format_go_float(sum.get()));
                let _ = write!(out, "{name}_count");
                write_labels(out, labels, None);
                let _ = writeln!(out, " {total}");
            }
        }
    }
}

/// `add_process_metrics`: the process collector's main series, read from `/proc/self` (absent
/// on systems without it).
fn render_process_metrics(out: &mut String) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |key: &str| -> Option<f64> {
        status
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse::<f64>().ok())
            .map(|n| n * 1024.0)
    };
    if let Some(rss) = kb("VmRSS:") {
        let _ = writeln!(
            out,
            "# HELP process_resident_memory_bytes Resident memory size in bytes."
        );
        let _ = writeln!(out, "# TYPE process_resident_memory_bytes gauge");
        let _ = writeln!(
            out,
            "process_resident_memory_bytes {}",
            format_go_float(rss)
        );
    }
    if let Some(vsz) = kb("VmSize:") {
        let _ = writeln!(
            out,
            "# HELP process_virtual_memory_bytes Virtual memory size in bytes."
        );
        let _ = writeln!(out, "# TYPE process_virtual_memory_bytes gauge");
        let _ = writeln!(out, "process_virtual_memory_bytes {}", format_go_float(vsz));
    }
    if let Ok(stat) = std::fs::read_to_string("/proc/self/stat") {
        // Fields after the command name (which may hold spaces) start at ") ": utime is field 14,
        // stime 15 of the whole line, so 12 and 13 after the state field. Ticks are 100 per second
        // on Linux (USER_HZ).
        let fields: Vec<&str> = stat
            .rsplit_once(") ")
            .map(|(_, rest)| rest.split_whitespace().collect())
            .unwrap_or_default();
        let ticks = |i: usize| fields.get(i).and_then(|f| f.parse::<f64>().ok());
        if let (Some(u), Some(s)) = (ticks(11), ticks(12)) {
            let _ = writeln!(
                out,
                "# HELP process_cpu_seconds_total Total user and system CPU time spent in seconds."
            );
            let _ = writeln!(out, "# TYPE process_cpu_seconds_total counter");
            let _ = writeln!(
                out,
                "process_cpu_seconds_total {}",
                format_go_float((u + s) / 100.0)
            );
        }
    }
    if let Ok(dir) = std::fs::read_dir("/proc/self/fd") {
        let n = dir.count();
        let _ = writeln!(
            out,
            "# HELP process_open_fds Number of open file descriptors."
        );
        let _ = writeln!(out, "# TYPE process_open_fds gauge");
        let _ = writeln!(out, "process_open_fds {n}");
    }
}

/// A view of the exporter with static labels and mappings (Bento's `metrics.Namespaced`): the
/// stream adds `stream` (and `org`), each component adds `label` and `path`.
#[derive(Clone)]
pub struct Namespaced {
    exporter: Arc<PromExporter>,
    labels: BTreeMap<String, String>,
    mappings: Vec<Arc<MetricMapping>>,
    owner: Option<SeriesOwner>,
}

impl Namespaced {
    /// A namespace over `exporter` holding its `metrics.mapping`, if any.
    pub fn new(exporter: Arc<PromExporter>) -> Self {
        let mapping = exporter.mapping();
        let mappings = if mapping.is_empty() {
            Vec::new()
        } else {
            vec![mapping]
        };
        Self {
            exporter,
            labels: BTreeMap::new(),
            mappings,
            owner: None,
        }
    }

    /// A namespace without the exporter's mapping (Bento's bare `NewNamespaced(child)`).
    pub fn bare(exporter: Arc<PromExporter>) -> Self {
        Self {
            exporter,
            labels: BTreeMap::new(),
            mappings: Vec::new(),
            owner: None,
        }
    }

    pub fn exporter(&self) -> &Arc<PromExporter> {
        &self.exporter
    }

    pub fn with_labels(&self, pairs: &[(&str, &str)]) -> Self {
        let mut next = self.clone();
        for (k, v) in pairs {
            next.labels.insert((*k).to_string(), (*v).to_string());
        }
        next
    }

    /// Adds a mapping that runs before the ones already held (Bento's `WithMapping`).
    pub fn with_mapping(&self, mapping: Arc<MetricMapping>) -> Self {
        let mut next = self.clone();
        next.mappings.insert(0, mapping);
        next
    }

    pub fn with_owner(&self, owner: SeriesOwner) -> Self {
        let mut next = self.clone();
        next.owner = Some(owner);
        next
    }

    fn path_and_labels(&self, path: &str) -> (String, Vec<String>, Vec<String>) {
        let mut names: Vec<String> = self.labels.keys().cloned().collect();
        let mut values: Vec<String> = self.labels.values().cloned().collect();
        let mut path = path.to_string();
        for m in &self.mappings {
            let (p, n, v) = m.map_path(&path, &names, &values);
            path = p;
            names = n;
            values = v;
            if path.is_empty() {
                break;
            }
        }
        (path, names, values)
    }

    fn handle(
        &self,
        path: &str,
        kind: FamilyKind,
        extra: &[(&str, &str)],
    ) -> Option<Arc<SeriesData>> {
        let (path, names, values) = self.path_and_labels(path);
        if path.is_empty() {
            return None;
        }
        let mut labels: Vec<(String, String)> = names.into_iter().zip(values).collect();
        for (k, v) in extra {
            labels.push(((*k).to_string(), (*v).to_string()));
        }
        self.exporter
            .get_or_create(&path, kind, labels, self.owner.as_ref())
    }

    pub fn counter(&self, path: &str) -> Counter {
        Counter(self.handle(path, FamilyKind::Counter, &[]))
    }

    pub fn gauge(&self, path: &str) -> Gauge {
        Gauge(self.handle(path, FamilyKind::Gauge, &[]))
    }

    pub fn timer(&self, path: &str) -> Timer {
        Timer(self.handle(path, FamilyKind::Timer, &[]))
    }

    /// A counter with dynamic labels named `label_names` after the static ones.
    pub fn counter_vec(&self, path: &str, label_names: &[&str]) -> MetricVec {
        self.vec(path, label_names, FamilyKind::Counter)
    }

    pub fn gauge_vec(&self, path: &str, label_names: &[&str]) -> MetricVec {
        self.vec(path, label_names, FamilyKind::Gauge)
    }

    pub fn timer_vec(&self, path: &str, label_names: &[&str]) -> MetricVec {
        self.vec(path, label_names, FamilyKind::Timer)
    }

    fn vec(&self, path: &str, label_names: &[&str], kind: FamilyKind) -> MetricVec {
        let (path, names, values) = self.path_and_labels(path);
        MetricVec {
            exporter: self.exporter.clone(),
            path,
            static_labels: names.into_iter().zip(values).collect(),
            names: label_names.iter().map(|s| (*s).to_string()).collect(),
            kind,
            owner: self.owner.clone(),
        }
    }
}

/// A family with dynamic labels (Bento's `GetCounterVec` and friends).
pub struct MetricVec {
    exporter: Arc<PromExporter>,
    path: String,
    static_labels: Vec<(String, String)>,
    names: Vec<String>,
    kind: FamilyKind,
    owner: Option<SeriesOwner>,
}

impl MetricVec {
    fn series(&self, values: &[&str]) -> Option<Arc<SeriesData>> {
        if self.path.is_empty() || values.len() != self.names.len() {
            return None;
        }
        let mut labels = self.static_labels.clone();
        for (k, v) in self.names.iter().zip(values.iter()) {
            labels.push((k.clone(), (*v).to_string()));
        }
        self.exporter
            .get_or_create(&self.path, self.kind, labels, self.owner.as_ref())
    }

    pub fn counter(&self, values: &[&str]) -> Counter {
        Counter(if self.kind == FamilyKind::Counter {
            self.series(values)
        } else {
            None
        })
    }

    pub fn gauge(&self, values: &[&str]) -> Gauge {
        Gauge(if self.kind == FamilyKind::Gauge {
            self.series(values)
        } else {
            None
        })
    }

    pub fn timer(&self, values: &[&str]) -> Timer {
        Timer(if self.kind == FamilyKind::Timer {
            self.series(values)
        } else {
            None
        })
    }
}
