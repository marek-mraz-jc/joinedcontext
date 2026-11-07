//! The app's own `/metrics` (T-3581): what the socket delivered, and how late.
//!
//! The latency is the hop the app sees: from the HFP position's `observedAt` to the moment its
//! notification arrived here, which is MQTT, the pipeline, the broker and the WebSocket
//! together. Quantiles cover the last ten minutes.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How far back the latency quantiles look.
const WINDOW: Duration = Duration::from_secs(600);
/// Samples kept at most: the fleet moves about once a second per vehicle.
const SAMPLES: usize = 100_000;

#[derive(Debug, Default)]
pub struct Metrics {
    notifications: AtomicU64,
    gaps: AtomicU64,
    reconnects: AtomicU64,
    latencies: Mutex<VecDeque<(Instant, u64)>>,
}

impl Metrics {
    pub fn notification(&self) {
        self.notifications.fetch_add(1, Ordering::Relaxed);
    }

    pub fn gap(&self) {
        self.gaps.fetch_add(1, Ordering::Relaxed);
    }

    pub fn reconnect(&self) {
        self.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    pub fn latency(&self, late: Duration) {
        let mut samples = self.latencies.lock().unwrap_or_else(|e| e.into_inner());
        if samples.len() >= SAMPLES {
            samples.pop_front();
        }
        samples.push_back((Instant::now(), late.as_millis() as u64));
    }

    /// The `q` quantile, in milliseconds, of the latencies inside the window.
    pub fn quantile(&self, q: f64) -> Option<u64> {
        let mut samples = self.latencies.lock().unwrap_or_else(|e| e.into_inner());
        while samples.front().is_some_and(|(at, _)| at.elapsed() > WINDOW) {
            samples.pop_front();
        }
        let mut sorted: Vec<u64> = samples.iter().map(|(_, ms)| *ms).collect();
        if sorted.is_empty() {
            return None;
        }
        sorted.sort_unstable();
        let at = ((sorted.len() - 1) as f64 * q).round() as usize;
        sorted.get(at).copied()
    }

    /// The Prometheus text of everything above; a quantile without samples is left out.
    pub fn text(&self, vehicles: usize) -> String {
        let mut out = format!(
            "# TYPE hsl_ws_notifications_total counter\nhsl_ws_notifications_total {}\n\
             # TYPE hsl_ws_gaps_total counter\nhsl_ws_gaps_total {}\n\
             # TYPE hsl_ws_reconnects_total counter\nhsl_ws_reconnects_total {}\n\
             # TYPE hsl_vehicles gauge\nhsl_vehicles {vehicles}\n\
             # TYPE hsl_update_latency_ms summary\n",
            self.notifications.load(Ordering::Relaxed),
            self.gaps.load(Ordering::Relaxed),
            self.reconnects.load(Ordering::Relaxed),
        );
        for q in [0.5, 0.95] {
            if let Some(ms) = self.quantile(q) {
                out.push_str(&format!("hsl_update_latency_ms{{quantile=\"{q}\"}} {ms}\n"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quantiles_read_the_samples_and_none_without_any() {
        let metrics = Metrics::default();
        assert_eq!(metrics.quantile(0.5), None);
        assert!(!metrics.text(0).contains("quantile"));
        for ms in 1..=100 {
            metrics.latency(Duration::from_millis(ms));
        }
        assert_eq!(metrics.quantile(0.5), Some(51));
        assert_eq!(metrics.quantile(0.95), Some(95));
        assert!(metrics.text(3).contains("hsl_vehicles 3\n"));
    }
}
