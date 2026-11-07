use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;
use chrono::Utc;
use tokio::sync::{mpsc, Notify};

use crate::connection::subject_matches;
use ekiben_core::CoreError;

#[derive(Clone, Debug)]
pub struct MemNatsMsg {
    pub subject: String,
    pub reply: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub data: Bytes,
    pub stream_seq: u64,
    pub timestamp_unix_nano: i64,
}

#[derive(Clone, Debug)]
pub struct JsDeliveredMsg {
    pub msg: MemNatsMsg,
    pub consumer_seq: u64,
    pub num_delivered: u64,
    pub num_pending: usize,
    pub domain: String,
}

#[derive(Clone, Debug)]
pub struct KvEntry {
    pub bucket: String,
    pub key: String,
    pub value: Bytes,
    pub revision: u64,
    pub delta: u64,
    pub operation: &'static str, // "KeyValuePutOp" | "KeyValueDeleteOp" | "KeyValuePurgeOp"
    pub created: String,
}

struct JsConsumerState {
    subject_filter: String,
    deliver_policy: String,
    max_ack_pending: usize,
    cursor_seq: u64,
    consumer_seq_counter: u64,
    in_flight: BTreeMap<u64, (MemNatsMsg, u64)>, // stream_seq -> (msg, delivery_count)
    redeliver_queue: VecDeque<(MemNatsMsg, u64)>,
    initialized: bool,
}

impl JsConsumerState {
    fn new(subject_filter: String, deliver_policy: String, max_ack_pending: usize) -> Self {
        Self {
            subject_filter,
            deliver_policy,
            max_ack_pending: max_ack_pending.max(1),
            cursor_seq: 0,
            consumer_seq_counter: 0,
            in_flight: BTreeMap::new(),
            redeliver_queue: VecDeque::new(),
            initialized: false,
        }
    }
}

struct CoreSubEntry {
    pattern: String,
    queue: Option<String>,
    tx: mpsc::Sender<MemNatsMsg>,
}

/// Self-contained in-memory NATS Core + JetStream + KV cluster used for `nats://memory*` URLs
/// so durable pull consumers, explicit Ack/Nak redelivery, `max_ack_pending` backpressure,
/// request-reply, and `nats_kv` can be tested deterministically in-process.
pub struct InMemoryNatsCluster {
    core_subs: Mutex<Vec<CoreSubEntry>>,
    rr_counter: Mutex<usize>,
    js_log: Mutex<Vec<MemNatsMsg>>,
    js_consumers: Mutex<HashMap<String, JsConsumerState>>,
    js_notify: Notify,
    kv_buckets: Mutex<HashMap<String, BTreeMap<String, Vec<KvEntry>>>>,
    kv_revisions: Mutex<HashMap<String, u64>>,
}

static GLOBAL_CLUSTERS: OnceLock<Mutex<HashMap<String, Arc<InMemoryNatsCluster>>>> =
    OnceLock::new();

impl InMemoryNatsCluster {
    pub fn get(url: &str) -> Arc<Self> {
        let map_mtx = GLOBAL_CLUSTERS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = map_mtx.lock().unwrap();
        guard
            .entry(url.to_string())
            .or_insert_with(|| {
                Arc::new(Self {
                    core_subs: Mutex::new(Vec::new()),
                    rr_counter: Mutex::new(0),
                    js_log: Mutex::new(Vec::new()),
                    js_consumers: Mutex::new(HashMap::new()),
                    js_notify: Notify::new(),
                    kv_buckets: Mutex::new(HashMap::new()),
                    kv_revisions: Mutex::new(HashMap::new()),
                })
            })
            .clone()
    }

    // Core NATS pub/sub
    pub fn subscribe_core(
        &self,
        pattern: &str,
        queue: Option<String>,
        cap: usize,
    ) -> mpsc::Receiver<MemNatsMsg> {
        let (tx, rx) = mpsc::channel(cap.max(64));
        self.core_subs.lock().unwrap().push(CoreSubEntry {
            pattern: pattern.to_string(),
            queue,
            tx,
        });
        rx
    }

    pub fn publish_core(
        &self,
        subject: String,
        reply: Option<String>,
        headers: BTreeMap<String, String>,
        data: Bytes,
    ) {
        let msg = MemNatsMsg {
            subject: subject.clone(),
            reply,
            headers,
            data,
            stream_seq: 0,
            timestamp_unix_nano: Utc::now().timestamp_nanos_opt().unwrap_or(0),
        };
        let mut subs = self.core_subs.lock().unwrap();
        subs.retain(|s| !s.tx.is_closed());

        // Deliver to all non-queue matching subscribers, and 1 per matching queue group
        let mut queue_groups: HashMap<String, Vec<usize>> = HashMap::new();
        for (idx, sub) in subs.iter().enumerate() {
            if subject_matches(&sub.pattern, &subject) {
                if let Some(ref q) = sub.queue {
                    queue_groups.entry(q.clone()).or_default().push(idx);
                } else {
                    let _ = sub.tx.try_send(msg.clone());
                }
            }
        }
        let mut rr = self.rr_counter.lock().unwrap();
        for indices in queue_groups.values() {
            if !indices.is_empty() {
                let chosen = indices[*rr % indices.len()];
                *rr = rr.wrapping_add(1);
                let _ = subs[chosen].tx.try_send(msg.clone());
            }
        }
    }

    // JetStream publish & durable consumer pull/ack/nak
    pub fn publish_jetstream(
        &self,
        subject: String,
        headers: BTreeMap<String, String>,
        data: Bytes,
    ) -> u64 {
        let mut log = self.js_log.lock().unwrap();
        let seq = (log.len() as u64) + 1;
        let msg = MemNatsMsg {
            subject: subject.clone(),
            reply: None,
            headers: headers.clone(),
            data: data.clone(),
            stream_seq: seq,
            timestamp_unix_nano: Utc::now().timestamp_nanos_opt().unwrap_or(0),
        };
        log.push(msg);
        drop(log);
        self.publish_core(subject, None, headers, data);
        self.js_notify.notify_waiters();
        seq
    }

    pub fn ensure_consumer(
        &self,
        consumer_key: &str,
        subject_filter: &str,
        deliver_policy: &str,
        max_ack_pending: usize,
    ) {
        let mut consumers = self.js_consumers.lock().unwrap();
        let log = self.js_log.lock().unwrap();
        let entry = consumers
            .entry(consumer_key.to_string())
            .or_insert_with(|| {
                JsConsumerState::new(
                    subject_filter.to_string(),
                    deliver_policy.to_string(),
                    max_ack_pending,
                )
            });
        entry.max_ack_pending = max_ack_pending.max(1);
        if !entry.initialized {
            entry.initialized = true;
            match entry.deliver_policy.as_str() {
                "new" => {
                    entry.cursor_seq = log.len() as u64;
                }
                "last" => {
                    entry.cursor_seq = (log.len() as u64).saturating_sub(1);
                }
                _ => {
                    entry.cursor_seq = 0;
                }
            }
        }
    }

    /// Attempts to pull the next message for `consumer_key`.
    /// Respects `max_ack_pending`: if `in_flight.len() >= max_ack_pending`, returns `None` (backpressure!).
    pub fn try_pull_jetstream(&self, consumer_key: &str) -> Option<JsDeliveredMsg> {
        let mut consumers = self.js_consumers.lock().unwrap();
        let log = self.js_log.lock().unwrap();
        let state = consumers.get_mut(consumer_key)?;

        // Backpressure check: halt consumption while max_ack_pending is reached!
        if state.in_flight.len() >= state.max_ack_pending {
            return None;
        }

        // 1. Check redelivery queue first (messages that were Nak'd!)
        if let Some((msg, prev_deliveries)) = state.redeliver_queue.pop_front() {
            let num_delivered = prev_deliveries + 1;
            state.consumer_seq_counter += 1;
            let c_seq = state.consumer_seq_counter;
            state
                .in_flight
                .insert(msg.stream_seq, (msg.clone(), num_delivered));
            let pending = log[state.cursor_seq as usize..]
                .iter()
                .filter(|m| subject_matches(&state.subject_filter, &m.subject))
                .count()
                + state.redeliver_queue.len();
            return Some(JsDeliveredMsg {
                msg,
                consumer_seq: c_seq,
                num_delivered,
                num_pending: pending,
                domain: "ekiben".to_string(),
            });
        }

        // 2. Scan forward in the stream log
        while (state.cursor_seq as usize) < log.len() {
            let msg = log[state.cursor_seq as usize].clone();
            state.cursor_seq += 1;
            if subject_matches(&state.subject_filter, &msg.subject) {
                state.consumer_seq_counter += 1;
                let c_seq = state.consumer_seq_counter;
                state.in_flight.insert(msg.stream_seq, (msg.clone(), 1));
                let pending = log[state.cursor_seq as usize..]
                    .iter()
                    .filter(|m| subject_matches(&state.subject_filter, &m.subject))
                    .count();
                return Some(JsDeliveredMsg {
                    msg,
                    consumer_seq: c_seq,
                    num_delivered: 1,
                    num_pending: pending,
                    domain: "ekiben".to_string(),
                });
            }
        }

        None
    }

    pub fn ack_jetstream(&self, consumer_key: &str, stream_seq: u64) {
        if let Some(state) = self.js_consumers.lock().unwrap().get_mut(consumer_key) {
            state.in_flight.remove(&stream_seq);
        }
        self.js_notify.notify_waiters();
    }

    pub fn nak_jetstream(&self, consumer_key: &str, stream_seq: u64) {
        if let Some(state) = self.js_consumers.lock().unwrap().get_mut(consumer_key) {
            if let Some((msg, deliveries)) = state.in_flight.remove(&stream_seq) {
                state.redeliver_queue.push_back((msg, deliveries));
            }
        }
        self.js_notify.notify_waiters();
    }

    pub async fn wait_for_js_activity(&self) {
        self.js_notify.notified().await;
    }

    // Key-Value Store operations
    fn next_kv_rev(&self, bucket: &str) -> u64 {
        let mut revs = self.kv_revisions.lock().unwrap();
        let entry = revs.entry(bucket.to_string()).or_insert(0);
        *entry += 1;
        *entry
    }

    pub fn kv_get(&self, bucket: &str, key: &str) -> Result<KvEntry, CoreError> {
        let buckets = self.kv_buckets.lock().unwrap();
        let Some(b_map) = buckets.get(bucket) else {
            return Err(CoreError::KeyNotFound(key.to_string()));
        };
        let Some(history) = b_map.get(key) else {
            return Err(CoreError::KeyNotFound(key.to_string()));
        };
        let Some(latest) = history.last() else {
            return Err(CoreError::KeyNotFound(key.to_string()));
        };
        if latest.operation != "KeyValuePutOp" {
            return Err(CoreError::KeyNotFound(key.to_string()));
        }
        Ok(latest.clone())
    }

    pub fn kv_get_revision(
        &self,
        bucket: &str,
        key: &str,
        revision: u64,
    ) -> Result<KvEntry, CoreError> {
        let buckets = self.kv_buckets.lock().unwrap();
        let Some(b_map) = buckets.get(bucket) else {
            return Err(CoreError::KeyNotFound(key.to_string()));
        };
        let Some(history) = b_map.get(key) else {
            return Err(CoreError::KeyNotFound(key.to_string()));
        };
        history
            .iter()
            .find(|e| e.revision == revision)
            .cloned()
            .ok_or_else(|| CoreError::KeyNotFound(format!("{key}@{revision}")))
    }

    pub fn kv_put(&self, bucket: &str, key: &str, value: Bytes) -> u64 {
        let rev = self.next_kv_rev(bucket);
        let mut buckets = self.kv_buckets.lock().unwrap();
        let b_map = buckets.entry(bucket.to_string()).or_default();
        let hist = b_map.entry(key.to_string()).or_default();
        hist.push(KvEntry {
            bucket: bucket.to_string(),
            key: key.to_string(),
            value,
            revision: rev,
            delta: 0,
            operation: "KeyValuePutOp",
            created: Utc::now().to_rfc3339(),
        });
        rev
    }

    pub fn kv_create(&self, bucket: &str, key: &str, value: Bytes) -> Result<u64, CoreError> {
        {
            let buckets = self.kv_buckets.lock().unwrap();
            if let Some(b_map) = buckets.get(bucket) {
                if let Some(hist) = b_map.get(key) {
                    if hist.last().is_some_and(|e| e.operation == "KeyValuePutOp") {
                        return Err(CoreError::KeyAlreadyExists(key.to_string()));
                    }
                }
            }
        }
        Ok(self.kv_put(bucket, key, value))
    }

    pub fn kv_update(
        &self,
        bucket: &str,
        key: &str,
        value: Bytes,
        expected_rev: u64,
    ) -> Result<u64, CoreError> {
        {
            let buckets = self.kv_buckets.lock().unwrap();
            let current_rev = buckets
                .get(bucket)
                .and_then(|b| b.get(key))
                .and_then(|h| h.last())
                .map(|e| e.revision)
                .unwrap_or(0);
            if current_rev != expected_rev {
                return Err(CoreError::other(format!(
                    "nats_kv update revision mismatch: expected {expected_rev}, got {current_rev}"
                )));
            }
        }
        Ok(self.kv_put(bucket, key, value))
    }

    pub fn kv_delete(&self, bucket: &str, key: &str) -> u64 {
        let rev = self.next_kv_rev(bucket);
        let mut buckets = self.kv_buckets.lock().unwrap();
        let b_map = buckets.entry(bucket.to_string()).or_default();
        let hist = b_map.entry(key.to_string()).or_default();
        hist.push(KvEntry {
            bucket: bucket.to_string(),
            key: key.to_string(),
            value: Bytes::new(),
            revision: rev,
            delta: 0,
            operation: "KeyValueDeleteOp",
            created: Utc::now().to_rfc3339(),
        });
        rev
    }

    pub fn kv_purge(&self, bucket: &str, key: &str) -> u64 {
        let rev = self.next_kv_rev(bucket);
        let mut buckets = self.kv_buckets.lock().unwrap();
        let b_map = buckets.entry(bucket.to_string()).or_default();
        let hist = b_map.entry(key.to_string()).or_default();
        hist.clear();
        hist.push(KvEntry {
            bucket: bucket.to_string(),
            key: key.to_string(),
            value: Bytes::new(),
            revision: rev,
            delta: 0,
            operation: "KeyValuePurgeOp",
            created: Utc::now().to_rfc3339(),
        });
        rev
    }

    pub fn kv_history(&self, bucket: &str, key_filter: &str) -> Vec<KvEntry> {
        let buckets = self.kv_buckets.lock().unwrap();
        let Some(b_map) = buckets.get(bucket) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (k, hist) in b_map {
            if subject_matches(key_filter, k) {
                out.extend(hist.iter().cloned());
            }
        }
        out
    }

    pub fn kv_keys(&self, bucket: &str, key_filter: &str) -> Vec<String> {
        let buckets = self.kv_buckets.lock().unwrap();
        let Some(b_map) = buckets.get(bucket) else {
            return Vec::new();
        };
        b_map
            .iter()
            .filter_map(|(k, hist)| {
                let is_live = hist.last().is_some_and(|e| e.operation == "KeyValuePutOp");
                (is_live && subject_matches(key_filter, k)).then_some(k.clone())
            })
            .collect()
    }
}
