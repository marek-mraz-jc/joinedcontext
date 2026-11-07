use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

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
        })
    }

    pub fn incr_counter(&self, name: &str, delta: u64) {
        if let Some(c) = self.counters.read().unwrap().get(name) {
            c.fetch_add(delta, Ordering::Relaxed);
            return;
        }
        let mut guard = self.counters.write().unwrap();
        let entry = guard
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)));
        entry.fetch_add(delta, Ordering::Relaxed);
    }

    pub fn record_timing(&self, name: &str, duration_ns: u64) {
        if let Some(t) = self.timings.read().unwrap().get(name) {
            t.lock().unwrap().record(duration_ns);
            return;
        }
        let mut guard = self.timings.write().unwrap();
        let entry = guard
            .entry(name.to_string())
            .or_insert_with(|| std::sync::Mutex::new(TimingReservoir::new(2048)));
        entry.get_mut().unwrap().record(duration_ns);
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
        for (k, v) in self.counters.read().unwrap().iter() {
            map.insert(k.clone(), v.load(Ordering::Relaxed).into());
        }
        for (k, v) in self.timings.read().unwrap().iter() {
            let stats = v.lock().unwrap().percentiles();
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
            other => self
                .counters
                .read()
                .unwrap()
                .get(other)
                .map(|c| c.load(Ordering::Relaxed))
                .unwrap_or(0),
        }
    }
}

/// Service-wide metrics registry exposing Prometheus text format on `/metrics`.
#[derive(Clone, Default)]
pub struct MetricsRegistry {
    streams: Arc<RwLock<BTreeMap<String, Arc<StreamMetrics>>>>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_stream(&self, metrics: Arc<StreamMetrics>) {
        self.streams
            .write()
            .unwrap()
            .insert(metrics.stream_id.clone(), metrics);
    }

    pub fn remove_stream(&self, stream_id: &str) {
        self.streams.write().unwrap().remove(stream_id);
    }

    pub fn render_prometheus(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "# HELP ekiben_input_received_total Total messages received by stream inputs."
        );
        let _ = writeln!(out, "# TYPE ekiben_input_received_total counter");
        let _ = writeln!(
            out,
            "# HELP ekiben_output_sent_total Total messages sent by stream outputs."
        );
        let _ = writeln!(out, "# TYPE ekiben_output_sent_total counter");
        let _ = writeln!(
            out,
            "# HELP ekiben_output_error_total Total output write errors."
        );
        let _ = writeln!(out, "# TYPE ekiben_output_error_total counter");
        let _ = writeln!(
            out,
            "# HELP ekiben_processor_received_total Total messages received per processor."
        );
        let _ = writeln!(out, "# TYPE ekiben_processor_received_total counter");
        let _ = writeln!(
            out,
            "# HELP ekiben_processor_sent_total Total messages sent per processor."
        );
        let _ = writeln!(out, "# TYPE ekiben_processor_sent_total counter");
        let _ = writeln!(
            out,
            "# HELP ekiben_processor_error_total Total processor errors."
        );
        let _ = writeln!(out, "# TYPE ekiben_processor_error_total counter");
        let _ = writeln!(
            out,
            "# HELP ekiben_processor_latency_ns Processor latency quantiles in nanoseconds."
        );
        let _ = writeln!(out, "# TYPE ekiben_processor_latency_ns summary");

        let guard = self.streams.read().unwrap();
        for (sid, sm) in guard.iter() {
            let org = &sm.org_id;
            let _ = writeln!(
                out,
                "ekiben_input_received_total{{org={org:?},stream={sid:?}}} {}",
                sm.input_received.load(Ordering::Relaxed)
            );
            let _ = writeln!(
                out,
                "ekiben_output_sent_total{{org={org:?},stream={sid:?}}} {}",
                sm.output_sent.load(Ordering::Relaxed)
            );
            let _ = writeln!(
                out,
                "ekiben_output_error_total{{org={org:?},stream={sid:?}}} {}",
                sm.output_error.load(Ordering::Relaxed)
            );

            for (k, v) in sm.counters.read().unwrap().iter() {
                let val = v.load(Ordering::Relaxed);
                if let Some((lbl, metric)) = k.rsplit_once('.') {
                    let prom_name = match metric {
                        "processor_received" => "ekiben_processor_received_total",
                        "processor_sent" => "ekiben_processor_sent_total",
                        "processor_error" => "ekiben_processor_error_total",
                        _ => continue,
                    };
                    let _ = writeln!(
                        out,
                        "{prom_name}{{org={org:?},stream={sid:?},label={lbl:?}}} {val}"
                    );
                }
            }

            for (k, v) in sm.timings.read().unwrap().iter() {
                if let Some((lbl, "processor_latency_ns")) = k.rsplit_once('.') {
                    let stats = v.lock().unwrap().percentiles();
                    let _ = writeln!(
                        out,
                        "ekiben_processor_latency_ns{{org={org:?},stream={sid:?},label={lbl:?},quantile=\"0.5\"}} {}",
                        stats.p50
                    );
                    let _ = writeln!(
                        out,
                        "ekiben_processor_latency_ns{{org={org:?},stream={sid:?},label={lbl:?},quantile=\"0.9\"}} {}",
                        stats.p90
                    );
                    let _ = writeln!(
                        out,
                        "ekiben_processor_latency_ns{{org={org:?},stream={sid:?},label={lbl:?},quantile=\"0.99\"}} {}",
                        stats.p99
                    );
                }
            }
        }
        out
    }
}
