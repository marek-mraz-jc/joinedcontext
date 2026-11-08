use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use crate::error::CoreError;
use crate::prometheus::{Counter, MetricsConfig, Namespaced, PromExporter, SeriesOwner};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TimingStats {
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
}

struct TimingReservoir {
    samples: Vec<u64>,
    max_samples: usize,
    cursor: usize,
}

impl TimingReservoir {
    fn new(max_samples: usize) -> Self {
        Self {
            samples: Vec::with_capacity(max_samples.min(1024)),
            max_samples,
            cursor: 0,
        }
    }

    fn record(&mut self, ns: u64) {
        if self.samples.len() < self.max_samples {
            self.samples.push(ns);
        } else {
            let idx = self.cursor % self.max_samples;
            self.samples[idx] = ns;
        }
        self.cursor = self.cursor.wrapping_add(1);
    }

    fn percentiles(&self) -> TimingStats {
        if self.samples.is_empty() {
            return TimingStats::default();
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let at = |q: f64| -> f64 {
            let idx = ((sorted.len() as f64 - 1.0) * q).round() as usize;
            sorted[idx.min(sorted.len() - 1)] as f64
        };
        TimingStats {
            p50: at(0.50),
            p90: at(0.90),
            p99: at(0.99),
        }
    }
}

/// Per-stream metrics collector (`internal/component/metrics/local.go`).
pub struct StreamMetrics {
    pub stream_id: String,
    pub org_id: String,
    pub input_received: AtomicU64,
    pub input_bytes: AtomicU64,
    pub output_sent: AtomicU64,
    pub output_batch_sent: AtomicU64,
    pub output_bytes: AtomicU64,
    pub output_error: AtomicU64,
    pub processor_errors_total: AtomicU64,
    counters: RwLock<BTreeMap<String, Arc<AtomicU64>>>,
    timings: RwLock<BTreeMap<String, std::sync::Mutex<TimingReservoir>>>,
    /// The stream's view of the Prometheus exporter (labels `stream` and, outside the default
    /// organization, `org`); `None` when the stream runs without a metrics registry.
    ns: RwLock<Option<Namespaced>>,
    /// `processor_error` counters of the stream's processors, by config path.
    proc_errors: RwLock<HashMap<String, Counter>>,
}

/// Bounds the per-stream counter and timing maps the stats API reads, against labels that
/// come from configs.
const MAX_LOCAL_SERIES: usize = 4096;

fn read_lock<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(PoisonError::into_inner)
}

fn write_lock<T>(l: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(PoisonError::into_inner)
}

impl StreamMetrics {
    pub fn new(stream_id: impl Into<String>, org_id: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            stream_id: stream_id.into(),
            org_id: org_id.into(),
            input_received: AtomicU64::new(0),
            input_bytes: AtomicU64::new(0),
            output_sent: AtomicU64::new(0),
            output_batch_sent: AtomicU64::new(0),
            output_bytes: AtomicU64::new(0),
            output_error: AtomicU64::new(0),
            processor_errors_total: AtomicU64::new(0),
            counters: RwLock::new(BTreeMap::new()),
            timings: RwLock::new(BTreeMap::new()),
            ns: RwLock::new(None),
            proc_errors: RwLock::new(HashMap::new()),
        })
    }

    /// Connects the stream to the exporter (done by `MetricsRegistry::register_stream`).
    pub fn attach(&self, ns: Namespaced) {
        *write_lock(&self.ns) = Some(ns);
    }

    /// The stream's namespace, if it is attached to an exporter.
    pub fn namespace(&self) -> Option<Namespaced> {
        read_lock(&self.ns).clone()
    }

    /// A component's namespace: Bento's `label` and `path` labels on top of the stream's.
    pub fn component(&self, label: &str, path: &str) -> Option<Namespaced> {
        self.namespace()
            .map(|ns| ns.with_labels(&[("label", label), ("path", path)]))
    }

    /// Records the `processor_error` counter of the processor at config path `path`.
    pub fn register_processor_error(&self, path: &str, counter: Counter) {
        let mut map = write_lock(&self.proc_errors);
        if map.len() < MAX_LOCAL_SERIES || map.contains_key(path) {
            map.insert(path.to_string(), counter);
        }
    }

    /// Counts `count` errors of the processor at `path` (`BatchProcContext.OnError`).
    pub fn incr_processor_error_at(&self, label: &str, path: &str, count: u64) {
        self.incr_processor_error(label, count);
        if let Some(c) = read_lock(&self.proc_errors).get(path) {
            c.incr(count);
        }
    }

    pub fn incr_counter(&self, name: &str, delta: u64) {
        if let Some(c) = read_lock(&self.counters).get(name) {
            c.fetch_add(delta, Ordering::Relaxed);
            return;
        }
        let mut guard = write_lock(&self.counters);
        if guard.len() >= MAX_LOCAL_SERIES && !guard.contains_key(name) {
            return;
        }
        let entry = guard
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)));
        entry.fetch_add(delta, Ordering::Relaxed);
    }

    pub fn record_timing(&self, name: &str, duration_ns: u64) {
        if let Some(t) = read_lock(&self.timings).get(name) {
            t.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .record(duration_ns);
            return;
        }
        let mut guard = write_lock(&self.timings);
        if guard.len() >= MAX_LOCAL_SERIES && !guard.contains_key(name) {
            return;
        }
        let entry = guard
            .entry(name.to_string())
            .or_insert_with(|| std::sync::Mutex::new(TimingReservoir::new(2048)));
        entry
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .record(duration_ns);
    }

    pub fn incr_processor_received(&self, label: &str, count: u64) {
        let key = if label.is_empty() {
            "processor_received".to_string()
        } else {
            format!("{label}.processor_received")
        };
        self.incr_counter(&key, count);
        let bkey = if label.is_empty() {
            "processor_batch_received".to_string()
        } else {
            format!("{label}.processor_batch_received")
        };
        self.incr_counter(&bkey, 1);
    }

    pub fn incr_processor_sent(&self, label: &str, count: u64, batches: u64) {
        let key = if label.is_empty() {
            "processor_sent".to_string()
        } else {
            format!("{label}.processor_sent")
        };
        self.incr_counter(&key, count);
        let bkey = if label.is_empty() {
            "processor_batch_sent".to_string()
        } else {
            format!("{label}.processor_batch_sent")
        };
        self.incr_counter(&bkey, batches);
    }

    pub fn incr_processor_error(&self, label: &str, count: u64) {
        self.processor_errors_total
            .fetch_add(count, Ordering::Relaxed);
        let key = if label.is_empty() {
            "processor_error".to_string()
        } else {
            format!("{label}.processor_error")
        };
        self.incr_counter(&key, count);
    }

    pub fn record_processor_latency(&self, label: &str, ns: u64) {
        let key = if label.is_empty() {
            "processor_latency_ns".to_string()
        } else {
            format!("{label}.processor_latency_ns")
        };
        self.record_timing(&key, ns);
    }

    pub fn snapshot_json(&self, uptime_ns: u64) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert(
            "input_received".to_string(),
            self.input_received.load(Ordering::Relaxed).into(),
        );
        map.insert(
            "output_sent".to_string(),
            self.output_sent.load(Ordering::Relaxed).into(),
        );
        map.insert(
            "output_batch_sent".to_string(),
            self.output_batch_sent.load(Ordering::Relaxed).into(),
        );
        map.insert(
            "output_error".to_string(),
            self.output_error.load(Ordering::Relaxed).into(),
        );
        for (k, v) in read_lock(&self.counters).iter() {
            map.insert(k.clone(), v.load(Ordering::Relaxed).into());
        }
        for (k, v) in read_lock(&self.timings).iter() {
            let stats = v
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .percentiles();
            map.insert(
                k.clone(),
                serde_json::to_value(stats).unwrap_or(serde_json::Value::Null),
            );
        }
        map.insert("uptime_ns".to_string(), uptime_ns.into());
        serde_json::Value::Object(map)
    }

    pub fn get_counter(&self, name: &str) -> u64 {
        match name {
            "input_received" => self.input_received.load(Ordering::Relaxed),
            "output_sent" => self.output_sent.load(Ordering::Relaxed),
            "output_batch_sent" => self.output_batch_sent.load(Ordering::Relaxed),
            "output_error" => self.output_error.load(Ordering::Relaxed),
            other => read_lock(&self.counters)
                .get(other)
                .map(|c| c.load(Ordering::Relaxed))
                .unwrap_or(0),
        }
    }
}

/// The organization whose series carry no `org` label (Bento's own label set).
pub const DEFAULT_ORG: &str = "default";

/// Running streams keyed by `(org, stream)`.
type StreamMap = BTreeMap<(String, String), Arc<StreamMetrics>>;

/// Service-wide metrics registry: the running streams and the Prometheus exporter behind
/// `/metrics` (Bento 1.22's metric names and labels).
#[derive(Clone)]
pub struct MetricsRegistry {
    streams: Arc<RwLock<StreamMap>>,
    exporter: Arc<PromExporter>,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self {
            streams: Arc::new(RwLock::new(BTreeMap::new())),
            exporter: Arc::new(PromExporter::with_defaults()),
        }
    }
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry exporting per `config` (the `metrics:` section); fails when its mapping does
    /// not parse.
    pub fn with_config(config: MetricsConfig) -> Result<Self, CoreError> {
        Ok(Self {
            streams: Arc::new(RwLock::new(BTreeMap::new())),
            exporter: Arc::new(PromExporter::new(config)?),
        })
    }

    pub fn exporter(&self) -> &Arc<PromExporter> {
        &self.exporter
    }

    /// Registers a stream and attaches it to the exporter. A stream id created again continues
    /// the counters its deleted predecessor left.
    pub fn register_stream(&self, metrics: Arc<StreamMetrics>) {
        let owner = SeriesOwner {
            org: metrics.org_id.clone(),
            stream: metrics.stream_id.clone(),
        };
        self.exporter.stream_created(&owner);
        let mut ns = Namespaced::new(self.exporter.clone());
        if metrics.org_id == DEFAULT_ORG {
            ns = ns.with_labels(&[("stream", metrics.stream_id.as_str())]);
        } else {
            ns = ns.with_labels(&[
                ("org", metrics.org_id.as_str()),
                ("stream", metrics.stream_id.as_str()),
            ]);
        }
        metrics.attach(ns.with_owner(owner));
        write_lock(&self.streams)
            .insert((metrics.org_id.clone(), metrics.stream_id.clone()), metrics);
    }

    /// Forgets every running stream named `stream_id` (in any organization) without marking it
    /// deleted for the exporter.
    pub fn remove_stream(&self, stream_id: &str) {
        write_lock(&self.streams).retain(|(_, sid), _| sid != stream_id);
    }

    /// A stream was deleted: it leaves the registry, its timings go, its counters stay.
    pub fn stream_deleted(&self, org: &str, stream_id: &str) {
        write_lock(&self.streams).remove(&(org.to_string(), stream_id.to_string()));
        self.exporter.stream_deleted(&SeriesOwner {
            org: org.to_string(),
            stream: stream_id.to_string(),
        });
    }

    /// The stream's live metrics, if it is registered.
    pub fn stream(&self, org: &str, stream_id: &str) -> Option<Arc<StreamMetrics>> {
        read_lock(&self.streams)
            .get(&(org.to_string(), stream_id.to_string()))
            .cloned()
    }

    /// Every series (the admin's view of `/metrics`).
    pub fn render_prometheus(&self) -> String {
        self.exporter.render(&|_| true)
    }

    /// The series of one organization's streams (an organization token's view of `/metrics`).
    pub fn render_prometheus_for_org(&self, org: &str) -> String {
        self.exporter
            .render(&|owner| owner.is_some_and(|o| o.org == org))
    }
}
