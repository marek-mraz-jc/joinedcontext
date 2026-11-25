use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;
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

/// Messages a memory cluster's JetStream log keeps; the oldest goes first, as JetStream's
/// limits policy drops it. Also the most a memory consumer or core subscription may hold
/// unacknowledged or queued (`max_ack_pending`, `prefetch_count`).
pub const MEM_JS_MAX_MSGS: usize = 10_000;
/// Bytes (subject, headers and payload) a memory cluster's JetStream log keeps.
pub const MEM_JS_MAX_BYTES: usize = 64 << 20;
/// Revisions a memory KV bucket keeps per key unless set otherwise (NATS KV's default).
pub const MEM_KV_DEFAULT_HISTORY: usize = 1;
/// The deepest history a memory KV bucket may keep (NATS KV's own limit).
pub const MEM_KV_MAX_HISTORY: usize = 64;
/// Bytes (keys and values of every kept revision, per entry overhead included) all KV buckets
/// of one memory cluster keep; a write past it is refused.
pub const MEM_KV_MAX_BYTES: usize = 64 << 20;
const KV_ENTRY_OVERHEAD: usize = 64;

/// The cluster refuses a config value over its cap, naming the field.
pub fn check_mem_cap(field: &str, value: usize) -> Result<(), CoreError> {
    if value > MEM_JS_MAX_MSGS {
        return Err(CoreError::config(format!(
            "{field} {value} is over the in-memory NATS cluster's limit of {MEM_JS_MAX_MSGS}"
        )));
    }
    Ok(())
}

fn msg_bytes(m: &MemNatsMsg) -> usize {
    m.subject.len()
        + m.data.len()
        + m.headers
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
}

fn kv_entry_bytes(e: &KvEntry) -> usize {
    e.key.len() + e.value.len() + KV_ENTRY_OVERHEAD
}

/// The JetStream log: `dropped` messages left the front, so `msgs[i]` has stream sequence
/// `dropped + i + 1`.
#[derive(Default)]
struct JsLog {
    msgs: VecDeque<MemNatsMsg>,
    dropped: u64,
    bytes: usize,
}

impl JsLog {
    fn end_seq(&self) -> u64 {
        self.dropped + self.msgs.len() as u64
    }

    /// The index of the first message after `cursor`; a cursor behind the front moves to it.
    fn index_after(&self, cursor: &mut u64) -> usize {
        *cursor = (*cursor).max(self.dropped);
        (*cursor - self.dropped) as usize
    }
}

#[derive(Default)]
struct KvState {
    buckets: HashMap<String, BTreeMap<String, VecDeque<KvEntry>>>,
    revisions: HashMap<String, u64>,
    history: HashMap<String, usize>,
    bytes: usize,
}

impl KvState {
    /// Appends `entry` to its key (after clearing the key when `clear`), trims the key to the
    /// bucket's history depth, and refuses the write when the cluster's KV bytes would pass
    /// [`MEM_KV_MAX_BYTES`].
    fn write(&mut self, entry: KvEntry, clear: bool) -> Result<u64, CoreError> {
        let depth = self
            .history
            .get(&entry.bucket)
            .copied()
            .unwrap_or(MEM_KV_DEFAULT_HISTORY);
        let hist = self
            .buckets
            .get(&entry.bucket)
            .and_then(|b| b.get(&entry.key));
        let kept = hist.map_or(0, VecDeque::len);
        let leaving = if clear {
            kept
        } else {
            (kept + 1).saturating_sub(depth)
        };
        let freed: usize = hist
            .into_iter()
            .flatten()
            .take(leaving)
            .map(kv_entry_bytes)
            .sum();
        let after = self.bytes - freed + kv_entry_bytes(&entry);
        if after > MEM_KV_MAX_BYTES {
            return Err(CoreError::other(format!(
                "nats: maximum bytes exceeded: the in-memory NATS cluster keeps at most {MEM_KV_MAX_BYTES} bytes of KV data"
            )));
        }
        let rev = self.revisions.entry(entry.bucket.clone()).or_insert(0);
        *rev += 1;
        let rev = *rev;
        let hist = self
            .buckets
            .entry(entry.bucket.clone())
            .or_default()
            .entry(entry.key.clone())
            .or_default();
        hist.drain(..leaving);
        hist.push_back(KvEntry {
            revision: rev,
            ..entry
        });
        self.bytes = after;
        Ok(rev)
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
    js_log: Mutex<JsLog>,
    js_consumers: Mutex<HashMap<String, JsConsumerState>>,
    js_notify: Notify,
    kv: Mutex<KvState>,
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
                    js_log: Mutex::new(JsLog::default()),
                    js_consumers: Mutex::new(HashMap::new()),
                    js_notify: Notify::new(),
                    kv: Mutex::new(KvState::default()),
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
        let (tx, rx) = mpsc::channel(cap.clamp(64, MEM_JS_MAX_MSGS));
        let mut subs = self.core_subs.lock().unwrap();
        subs.retain(|s| !s.tx.is_closed());
        subs.push(CoreSubEntry {
            pattern: pattern.to_string(),
            queue,
            tx,
        });
        rx
    }

    /// Core subscriptions the cluster holds, closed ones included until the next subscribe or
    /// publish prunes them.
    pub fn core_subscriptions(&self) -> usize {
        self.core_subs.lock().unwrap().len()
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
            timestamp_unix_nano: i64::try_from(jiff::Timestamp::now().as_nanosecond()).unwrap_or(0),
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
        // A group exists only once a member matched, so it is never empty.
        for indices in queue_groups.values() {
            let chosen = indices[*rr % indices.len()];
            *rr = rr.wrapping_add(1);
            let _ = subs[chosen].tx.try_send(msg.clone());
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
        let seq = log.end_seq() + 1;
        let msg = MemNatsMsg {
            subject: subject.clone(),
            reply: None,
            headers: headers.clone(),
            data: data.clone(),
            stream_seq: seq,
            timestamp_unix_nano: i64::try_from(jiff::Timestamp::now().as_nanosecond()).unwrap_or(0),
        };
        log.bytes += msg_bytes(&msg);
        log.msgs.push_back(msg);
        // ponytail: one message larger than MEM_JS_MAX_BYTES stays until the next publish.
        while let Some(old) = (log.msgs.len() > 1
            && (log.msgs.len() > MEM_JS_MAX_MSGS || log.bytes > MEM_JS_MAX_BYTES))
            .then(|| log.msgs.pop_front())
            .flatten()
        {
            log.bytes -= msg_bytes(&old);
            log.dropped += 1;
        }
        drop(log);
        self.publish_core(subject, None, headers, data);
        self.js_notify.notify_waiters();
        seq
    }

    /// Messages and bytes the JetStream log keeps.
    pub fn js_retained(&self) -> (usize, usize) {
        let log = self.js_log.lock().unwrap();
        (log.msgs.len(), log.bytes)
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
                    entry.cursor_seq = log.end_seq();
                }
                "last" => {
                    entry.cursor_seq = log.end_seq().saturating_sub(1);
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
            let start = log.index_after(&mut state.cursor_seq);
            let pending = log
                .msgs
                .range(start..)
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
        let mut idx = log.index_after(&mut state.cursor_seq);
        while let Some(msg) = log.msgs.get(idx).cloned() {
            idx += 1;
            state.cursor_seq += 1;
            if subject_matches(&state.subject_filter, &msg.subject) {
                state.consumer_seq_counter += 1;
                let c_seq = state.consumer_seq_counter;
                state.in_flight.insert(msg.stream_seq, (msg.clone(), 1));
                let pending = log
                    .msgs
                    .range(idx..)
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

    /// Sets how many revisions per key `bucket` keeps (1 to [`MEM_KV_MAX_HISTORY`]), as a
    /// bucket's `history` does on a NATS server.
    pub fn kv_set_history(&self, bucket: &str, depth: usize) -> Result<(), CoreError> {
        if !(1..=MEM_KV_MAX_HISTORY).contains(&depth) {
            return Err(CoreError::config(format!(
                "history {depth} must be between 1 and {MEM_KV_MAX_HISTORY}"
            )));
        }
        self.kv
            .lock()
            .unwrap()
            .history
            .insert(bucket.to_string(), depth);
        Ok(())
    }

    /// Bytes all KV buckets of the cluster keep, as [`MEM_KV_MAX_BYTES`] counts them.
    pub fn kv_retained_bytes(&self) -> usize {
        self.kv.lock().unwrap().bytes
    }

    fn kv_latest(&self, bucket: &str, key: &str) -> Option<KvEntry> {
        let kv = self.kv.lock().unwrap();
        kv.buckets.get(bucket)?.get(key)?.back().cloned()
    }

    fn kv_write(
        &self,
        bucket: &str,
        key: &str,
        value: Bytes,
        operation: &'static str,
    ) -> Result<u64, CoreError> {
        let entry = KvEntry {
            bucket: bucket.to_string(),
            key: key.to_string(),
            value,
            revision: 0,
            delta: 0,
            operation,
            created: crate::kv_backend::go_rfc3339_nano(jiff::Timestamp::now()),
        };
        self.kv
            .lock()
            .unwrap()
            .write(entry, operation == "KeyValuePurgeOp")
    }

    pub fn kv_get(&self, bucket: &str, key: &str) -> Result<KvEntry, CoreError> {
        // A key's history is never empty: a delete or purge leaves its marker.
        match self.kv_latest(bucket, key) {
            Some(latest) if latest.operation == "KeyValuePutOp" => Ok(latest),
            _ => Err(CoreError::KeyNotFound(key.to_string())),
        }
    }

    pub fn kv_get_revision(
        &self,
        bucket: &str,
        key: &str,
        revision: u64,
    ) -> Result<KvEntry, CoreError> {
        let kv = self.kv.lock().unwrap();
        let Some(history) = kv.buckets.get(bucket).and_then(|b| b.get(key)) else {
            return Err(CoreError::KeyNotFound(key.to_string()));
        };
        history
            .iter()
            .find(|e| e.revision == revision)
            .cloned()
            .ok_or_else(|| CoreError::KeyNotFound(format!("{key}@{revision}")))
    }

    pub fn kv_put(&self, bucket: &str, key: &str, value: Bytes) -> Result<u64, CoreError> {
        self.kv_write(bucket, key, value, "KeyValuePutOp")
    }

    pub fn kv_create(&self, bucket: &str, key: &str, value: Bytes) -> Result<u64, CoreError> {
        if self
            .kv_latest(bucket, key)
            .is_some_and(|e| e.operation == "KeyValuePutOp")
        {
            return Err(CoreError::KeyAlreadyExists(key.to_string()));
        }
        self.kv_put(bucket, key, value)
    }

    pub fn kv_update(
        &self,
        bucket: &str,
        key: &str,
        value: Bytes,
        expected_rev: u64,
    ) -> Result<u64, CoreError> {
        let current_rev = self.kv_latest(bucket, key).map_or(0, |e| e.revision);
        if current_rev != expected_rev {
            return Err(CoreError::other(format!(
                "nats_kv update revision mismatch: expected {expected_rev}, got {current_rev}"
            )));
        }
        self.kv_put(bucket, key, value)
    }

    pub fn kv_delete(&self, bucket: &str, key: &str) -> Result<u64, CoreError> {
        self.kv_write(bucket, key, Bytes::new(), "KeyValueDeleteOp")
    }

    pub fn kv_purge(&self, bucket: &str, key: &str) -> Result<u64, CoreError> {
        self.kv_write(bucket, key, Bytes::new(), "KeyValuePurgeOp")
    }

    pub fn kv_history(&self, bucket: &str, key_filter: &str) -> Vec<KvEntry> {
        let kv = self.kv.lock().unwrap();
        let Some(b_map) = kv.buckets.get(bucket) else {
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
        let kv = self.kv.lock().unwrap();
        let Some(b_map) = kv.buckets.get(bucket) else {
            return Vec::new();
        };
        b_map
            .iter()
            .filter_map(|(k, hist)| {
                let is_live = hist.back().is_some_and(|e| e.operation == "KeyValuePutOp");
                (is_live && subject_matches(key_filter, k)).then_some(k.clone())
            })
            .collect()
    }
}
