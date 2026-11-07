//! The last messages through each node of a running stream (T-3974, API/01 §7b, PL-66): the
//! studio's live provenance, NiFi's Data Provenance in a pipeline's own terms.
//!
//! The runner taps the stream after its source, after each step and before the write; each tap
//! is a branch whose answer is dropped and whose failure is caught inside it, so the Portal being
//! away never costs the stream a message. It sends the first [`TAP_FIRST`] messages after the
//! runner starts and then one in [`TAP_EVERY`]. The Portal keeps the newest [`KEPT`] per node in
//! memory for [`MAX_AGE_SECS`]; a read takes out what the target Endpoint hides and masks every
//! key of the secret list.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

/// The prefix of a tap's labels, which the metrics leave out like the outcome sink.
pub const TAP_PREFIX: &str = "tap_";
/// Messages a tap sends after the runner starts, before it sends one in [`TAP_EVERY`].
pub const TAP_FIRST: u64 = 20;
/// ponytail: a fixed one-in-a-hundred after the first twenty; a per-pipeline rate if a busy
/// stream's reports cost the Portal more than they show.
pub const TAP_EVERY: u64 = 100;
/// Samples kept per node.
pub const KEPT: usize = 20;
/// The largest record a sample keeps, as JSON; a larger one is kept as a note of its size.
pub const MAX_RECORD_BYTES: usize = 8 * 1024;
/// How long a sample is shown.
pub const MAX_AGE_SECS: i64 = 24 * 3600;
/// Nodes one pipeline keeps samples of, so a runner cannot grow the map without bound.
const MAX_NODES: usize = 64;

/// The node names a tap reports, the labels of the metrics' `nodes`.
pub fn is_node(node: &str) -> bool {
    let numbered = |prefix: &str| {
        node.strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.len() <= 4 && n.bytes().all(|b| b.is_ascii_digit()))
    };
    matches!(node, "input" | "output" | "compute") || numbered("step_") || numbered("compute_")
}

/// One tap: a branch that sends a sample of the message to `url` and keeps nothing of the answer.
fn tap(node: &str, url: &str, stream_id: &str) -> Value {
    let counter =
        serde_json::to_string(&format!("{TAP_PREFIX}{stream_id}/{node}")).unwrap_or_default();
    let node_json = serde_json::to_string(node).unwrap_or_default();
    json!({
        "label": format!("{TAP_PREFIX}{node}"),
        "branch": {
            // `this` fails on a message that is no JSON (a CSV line at the source); its text is
            // sent instead, so the request map never flags the message itself.
            "request_map": format!(
                "let n = count({counter})\nroot = if $n <= {TAP_FIRST} || $n % {TAP_EVERY} == 0 {{ {{\"node\": {node_json}, \"record\": this.catch(content().string())}} }} else {{ deleted() }}"
            ),
            "processors": [
                { "label": format!("{TAP_PREFIX}{node}_send"), "http": {
                    "url": url,
                    "verb": "POST",
                    "headers": { "Content-Type": "application/json" },
                    "timeout": "2s",
                    "retries": 0,
                    "oauth2": {
                        "enabled": true,
                        "client_key": "${JC_CLIENT_ID}",
                        "client_secret": "${JC_CLIENT_SECRET}",
                        "token_url": "${JC_TOKEN_URL}",
                    },
                }},
                // A refused or unanswered report is cleared inside the branch, never on the message.
                { "catch": [] }
            ],
        }
    })
}

/// Taps a rendered stream: after the source, after every `step_{j}` and `compute` processor, and
/// before the write. A stream without a processor list gets the source and output taps alone.
pub fn with_taps(stream: &mut Value, url: &str, stream_id: &str) {
    let Some(object) = stream.as_object_mut() else {
        return;
    };
    let pipeline = object.entry("pipeline").or_insert_with(|| json!({}));
    let Some(pipeline) = pipeline.as_object_mut() else {
        return;
    };
    let Some(processors) = pipeline
        .entry("processors")
        .or_insert_with(|| json!([]))
        .as_array_mut()
    else {
        return;
    };
    let mut tapped = Vec::with_capacity(processors.len() * 2 + 2);
    tapped.push(tap("input", url, stream_id));
    for processor in processors.drain(..) {
        let node = processor
            .get("label")
            .and_then(Value::as_str)
            .filter(|label| *label != "input" && *label != "output" && is_node(label))
            .map(str::to_owned);
        tapped.push(processor);
        if let Some(node) = node {
            tapped.push(tap(&node, url, stream_id));
        }
    }
    tapped.push(tap("output", url, stream_id));
    *processors = tapped;
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Sample {
    #[schema(value_type = String, format = DateTime)]
    pub at: DateTime<Utc>,
    #[schema(value_type = Object)]
    pub record: Value,
}

type Key = (String, String);

/// The newest samples per pipeline and node, in memory: a restart forgets them, as it should.
#[derive(Default)]
pub struct SampleStore {
    inner: RwLock<HashMap<Key, BTreeMap<String, VecDeque<Sample>>>>,
}

impl SampleStore {
    pub fn push(&self, project: &str, pipeline: &str, node: &str, record: Value) {
        let size = serde_json::to_vec(&record).map_or(usize::MAX, |bytes| bytes.len());
        let record = if size > MAX_RECORD_BYTES {
            json!({ "tooLarge": size })
        } else {
            record
        };
        let Ok(mut map) = self.inner.write() else {
            return;
        };
        let nodes = map
            .entry((project.to_owned(), pipeline.to_owned()))
            .or_default();
        if !nodes.contains_key(node) && nodes.len() >= MAX_NODES {
            return;
        }
        let kept = nodes.entry(node.to_owned()).or_default();
        if kept.len() == KEPT {
            kept.pop_front();
        }
        kept.push_back(Sample {
            at: Utc::now(),
            record,
        });
    }

    /// The samples of one pipeline younger than [`MAX_AGE_SECS`], newest first per node.
    pub fn read(
        &self,
        project: &str,
        pipeline: &str,
        now: DateTime<Utc>,
    ) -> BTreeMap<String, Vec<Sample>> {
        let Ok(map) = self.inner.read() else {
            return BTreeMap::new();
        };
        let Some(nodes) = map.get(&(project.to_owned(), pipeline.to_owned())) else {
            return BTreeMap::new();
        };
        nodes
            .iter()
            .map(|(node, kept)| {
                let fresh: Vec<Sample> = kept
                    .iter()
                    .rev()
                    .filter(|sample| (now - sample.at).num_seconds() < MAX_AGE_SECS)
                    .cloned()
                    .collect();
                (node.clone(), fresh)
            })
            .filter(|(_, fresh)| !fresh.is_empty())
            .collect()
    }
}

/// A sample as a reader may see it: the target's hidden attributes out, secret-list keys masked.
pub fn redacted(mut record: Value, hidden: &[String]) -> Value {
    if let Some(fields) = record.as_object_mut() {
        for name in hidden {
            fields.remove(name);
        }
    }
    mask(&mut record);
    record
}

fn mask(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, inner) in fields.iter_mut() {
                if crate::secret_scan::secret_name(key) && !inner.is_object() {
                    *inner = json!("•••");
                } else {
                    mask(inner);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask),
        Value::String(text) => *text = crate::pipeline_outcomes::mask_text(text),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_is_tapped_after_its_source_each_step_and_before_its_write() {
        let mut stream = json!({ "input": { "label": "input" }, "pipeline": { "processors": [
            { "label": "processor_0", "mapping": "root = this" },
            { "label": "step_0", "mapping": "root = this" },
            { "label": "compute", "mapping": "root = this" },
        ]}, "output": { "label": "output" } });
        with_taps(
            &mut stream,
            "http://p/internal/pipelines/a/b/samples",
            "a.b",
        );
        let labels: Vec<&str> = stream["pipeline"]["processors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["label"].as_str().unwrap())
            .collect();
        assert_eq!(
            labels,
            [
                "tap_input",
                "processor_0",
                "step_0",
                "tap_step_0",
                "compute",
                "tap_compute",
                "tap_output"
            ]
        );
        let tap = &stream["pipeline"]["processors"][0]["branch"];
        assert!(tap["request_map"]
            .as_str()
            .unwrap()
            .contains("count(\"tap_a.b/input\")"));
        assert_eq!(tap["processors"][1], json!({ "catch": [] }));
        assert!(
            tap.get("result_map").is_none(),
            "the answer is never mapped back"
        );
        // A stream with no processors still shows what it read and what it wrote.
        let mut bare = json!({ "input": {}, "output": {} });
        with_taps(&mut bare, "http://p", "a.b");
        assert_eq!(bare["pipeline"]["processors"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_node_is_one_the_metrics_name() {
        for ok in [
            "input",
            "output",
            "compute",
            "compute_1",
            "step_0",
            "step_12",
        ] {
            assert!(is_node(ok), "{ok}");
        }
        for bad in [
            "",
            "step_",
            "step_x",
            "processor_0",
            "tap_input",
            "../x",
            "step_99999",
        ] {
            assert!(!is_node(bad), "{bad}");
        }
    }

    #[test]
    fn the_store_keeps_the_newest_per_node_drops_old_and_notes_a_large_record() {
        let store = SampleStore::default();
        for i in 0..(KEPT + 5) {
            store.push("hel", "air", "step_0", json!({ "i": i }));
        }
        store.push(
            "hel",
            "air",
            "input",
            json!({ "blob": "x".repeat(MAX_RECORD_BYTES) }),
        );
        let read = store.read("hel", "air", Utc::now());
        assert_eq!(read["step_0"].len(), KEPT);
        assert_eq!(
            read["step_0"][0].record,
            json!({ "i": KEPT + 4 }),
            "newest first"
        );
        assert!(read["input"][0].record["tooLarge"].as_u64().unwrap() > MAX_RECORD_BYTES as u64);
        assert!(store.read("hel", "other", Utc::now()).is_empty());
        assert!(store.read("esp", "air", Utc::now()).is_empty());
        let later = Utc::now() + chrono::Duration::seconds(MAX_AGE_SECS + 1);
        assert!(store.read("hel", "air", later).is_empty(), "samples expire");
    }

    #[test]
    fn a_reader_sees_no_hidden_attribute_and_no_secret() {
        let record = json!({
            "id": "urn:ngsi-ld:X:1",
            "contactPoint": { "value": "jana@hel.fi" },
            "auth": { "password": "hunter2", "token": { "name": "ref" } },
            "list": [{ "apiKey": "abc" }],
        });
        let seen = redacted(record, &["contactPoint".to_owned()]);
        assert!(seen.get("contactPoint").is_none());
        assert_eq!(seen["auth"]["password"], json!("•••"));
        assert_eq!(
            seen["auth"]["token"],
            json!({ "name": "ref" }),
            "a reference stays"
        );
        assert!(!seen.to_string().contains("hunter2"));
    }
}
