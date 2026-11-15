//! The `ngsi_ld` metrics, kept in the stream's counters and timings under `<label>.<name>` keys:
//! `ngsi_ld_requests_total{status="<code>|transport"}`, `ngsi_ld_entities_written_total`,
//! `ngsi_ld_entities_refused_total`, `ngsi_ld_entities_read_total` and the timing
//! `ngsi_ld_request_latency_ns`. Each is also exported on `/metrics` (E2) under its own name with
//! a `label` (and `status`) label beside the stream's own labels. A status is digits or
//! `transport`, never text from the broker, so no answer can forge a series.

use ekiben_core::StreamMetrics;

pub const REQUESTS: &str = "ngsi_ld_requests_total";
pub const WRITTEN: &str = "ngsi_ld_entities_written_total";
pub const REFUSED: &str = "ngsi_ld_entities_refused_total";
pub const READ: &str = "ngsi_ld_entities_read_total";
pub const LATENCY: &str = "ngsi_ld_request_latency_ns";

fn label_or(label: &str) -> &str {
    if label.is_empty() {
        "ngsi_ld"
    } else {
        label
    }
}

pub fn requests_key(label: &str, status: &str) -> String {
    format!("{}.{REQUESTS}{{status=\"{status}\"}}", label_or(label))
}

pub fn written_key(label: &str) -> String {
    format!("{}.{WRITTEN}", label_or(label))
}

pub fn refused_key(label: &str) -> String {
    format!("{}.{REFUSED}", label_or(label))
}

pub fn read_key(label: &str) -> String {
    format!("{}.{READ}", label_or(label))
}

pub fn latency_key(label: &str) -> String {
    format!("{}.{LATENCY}", label_or(label))
}

/// Adds `delta` to the counter `name` (`status` for [`REQUESTS`]): the stream's stats key and,
/// when the stream is attached to an exporter, its `/metrics` series.
pub fn incr(m: &StreamMetrics, label: &str, name: &str, status: Option<&str>, delta: u64) {
    let label = label_or(label);
    let key = match status {
        Some(s) => format!("{label}.{name}{{status=\"{s}\"}}"),
        None => format!("{label}.{name}"),
    };
    m.incr_counter(&key, delta);
    if let Some(ns) = m.namespace() {
        let ns = ns.with_labels(&[("label", label)]);
        let counter = match status {
            Some(s) => ns.with_labels(&[("status", s)]).counter(name),
            None => ns.counter(name),
        };
        counter.incr(delta);
    }
}

/// Records one request's latency in the stream's stats and on `/metrics`.
pub fn timing(m: &StreamMetrics, label: &str, ns: u64) {
    m.record_timing(&latency_key(label), ns);
    if let Some(space) = m.namespace() {
        space
            .with_labels(&[("label", label_or(label))])
            .timer(LATENCY)
            .timing(ns);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekiben_core::MetricsRegistry;

    #[test]
    fn counters_reach_the_stats_and_the_exporter() {
        let reg = MetricsRegistry::new();
        let m = StreamMetrics::new("s1", "default");
        reg.register_stream(m.clone());
        incr(&m, "w", WRITTEN, None, 3);
        incr(&m, "w", REQUESTS, Some("207"), 1);
        timing(&m, "w", 1_000);
        assert_eq!(m.get_counter("w.ngsi_ld_entities_written_total"), 3);
        assert_eq!(m.get_counter("w.ngsi_ld_requests_total{status=\"207\"}"), 1);
        let text = reg.render_prometheus();
        assert!(
            text.contains("ngsi_ld_entities_written_total{label=\"w\",stream=\"s1\"} 3"),
            "{text}"
        );
        assert!(
            text.contains("ngsi_ld_requests_total{label=\"w\",status=\"207\",stream=\"s1\"} 1"),
            "{text}"
        );
        assert!(text.contains("ngsi_ld_request_latency_ns"), "{text}");
    }

    #[test]
    fn a_stream_without_an_exporter_still_counts() {
        let m = StreamMetrics::new("s2", "default");
        incr(&m, "", READ, None, 2);
        assert_eq!(m.get_counter("ngsi_ld.ngsi_ld_entities_read_total"), 2);
    }
}
