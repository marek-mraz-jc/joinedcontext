//! The `ngsi_ld` metrics, kept in the stream's counters and timings under `<label>.<name>` keys:
//! `ngsi_ld_requests_total{status="<code>|transport"}`, `ngsi_ld_entities_written_total`,
//! `ngsi_ld_entities_refused_total`, `ngsi_ld_entities_read_total` and the timing
//! `ngsi_ld_request_latency_ns`. E2's exporter renders them with the `label`, `path` and `stream`
//! labels; until it lands they show in the stream's stats. A status is digits or `transport`,
//! never text from the broker, so no answer can forge a series.

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
