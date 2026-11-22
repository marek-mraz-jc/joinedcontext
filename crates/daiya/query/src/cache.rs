#![forbid(unsafe_code)]
//! The query cache (R3, R7, §2.6, §4.3 CVE-2022-23498, §5.3 test 5), a [`BoundedCache`]:
//! - Keyed by `(org_id, instance_id, subject_scope_hash, datasource_uid, datasource_revision, canonical_query_hash, aligned_from_ms, aligned_to_ms, interval_ms)`.
//! - Public dashboard cache key: `(org_id, instance_id, dashboard_uid, dashboard_version, revision, panel_id, from_bucket_ms, to_bucket_ms)`.
//!   daiya has no project id, so neither key carries one.
//! - Never shares cache entries across organizations or across different subject permission scopes!
//! - One byte budget per process, whatever the number of organizations, and per-datasource
//!   hit/miss/byte statistics (`QueryCacheStatRecord`).

use daiya_model::{CanonicalJson, FrameSet, InstanceId, OrgId, QueryCacheStatRecord, sha256_hex};
use jc_types::{BoundedCache, OrgKeyed};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// Cache key for authenticated datasource queries (§2.6, §4.3 CVE-2022-23498).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheKey {
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub subject_scope_hash: String,
    pub datasource_uid: String,
    /// The datasource's settings, hashed: a changed datasource never answers from the old one.
    pub datasource_revision: String,
    pub canonical_query_hash: String,
    pub aligned_from_ms: i64,
    pub aligned_to_ms: i64,
    pub interval_ms: u64,
}

impl CacheKey {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        org_id: OrgId,
        instance_id: InstanceId,
        subject_principals: &[&str],
        datasource_uid: &str,
        datasource_revision: &str,
        query_json: &Value,
        from_ms: i64,
        to_ms: i64,
        interval_ms: u64,
    ) -> Self {
        let mut sorted_principals = subject_principals.to_vec();
        sorted_principals.sort_unstable();
        let subject_scope_hash = sha256_hex(sorted_principals.join("\u{0}").as_bytes());

        let canonical = CanonicalJson::canonicalize(query_json);
        let canonical_bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        let canonical_query_hash = sha256_hex(&canonical_bytes);

        let step = (interval_ms.max(1_000)) as i64;
        let aligned_from_ms = from_ms.div_euclid(step) * step;
        let aligned_to_ms = to_ms.div_euclid(step) * step;

        Self {
            org_id,
            instance_id,
            subject_scope_hash,
            datasource_uid: datasource_uid.to_owned(),
            datasource_revision: datasource_revision.to_owned(),
            canonical_query_hash,
            aligned_from_ms,
            aligned_to_ms,
            interval_ms,
        }
    }
}

/// Cache key for anonymous public dashboard panel queries (R7: per `(org_id, dashboard_uid,
/// dashboard_version, panel_id, range_bucket)`), plus the interval and `maxDataPoints` the
/// anonymous caller chose, so one caller's coarse request is never served to the next.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicCacheKey {
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub dashboard_uid: String,
    pub dashboard_version: i32,
    /// The panel's datasources and what its sanitisation depends on, hashed.
    pub revision: String,
    pub panel_id: u32,
    pub from_bucket_ms: i64,
    pub to_bucket_ms: i64,
    pub interval_ms: i64,
    pub max_data_points: u32,
}

impl PublicCacheKey {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        org_id: OrgId,
        instance_id: InstanceId,
        dashboard_uid: impl Into<String>,
        dashboard_version: i32,
        revision: &str,
        panel_id: u32,
        from_ms: i64,
        to_ms: i64,
        bucket_ms: i64,
        max_data_points: u32,
    ) -> Self {
        let b = bucket_ms.max(1_000);
        Self {
            org_id,
            instance_id,
            dashboard_uid: dashboard_uid.into(),
            dashboard_version,
            revision: revision.to_owned(),
            panel_id,
            from_bucket_ms: from_ms.div_euclid(b) * b,
            to_bucket_ms: to_ms.div_euclid(b) * b,
            interval_ms: b,
            max_data_points,
        }
    }
}

/// The two kinds of key share one budget.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Query(CacheKey),
    Public(PublicCacheKey),
}

impl Key {
    fn instance_id(&self) -> InstanceId {
        match self {
            Self::Query(key) => key.instance_id,
            Self::Public(key) => key.instance_id,
        }
    }

    /// What the statistics count this key under.
    fn source(&self) -> String {
        match self {
            Self::Query(key) => key.datasource_uid.clone(),
            Self::Public(key) => format!("public:{}", key.dashboard_uid),
        }
    }
}

impl OrgKeyed for Key {
    type Org = OrgId;

    fn org_id(&self) -> OrgId {
        match self {
            Self::Query(key) => key.org_id,
            Self::Public(key) => key.org_id,
        }
    }

    fn heap_bytes(&self) -> usize {
        match self {
            Self::Query(key) => {
                key.subject_scope_hash.len()
                    + key.datasource_uid.len()
                    + key.datasource_revision.len()
                    + key.canonical_query_hash.len()
            }
            Self::Public(key) => key.dashboard_uid.len() + key.revision.len(),
        }
    }
}

/// What a value holds on the heap. Its JSON is a third of that for columns of numbers.
fn value_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        Value::Array(items) => items
            .iter()
            .map(|item| size_of::<Value>() + value_bytes(item))
            .sum(),
        Value::Object(members) => members
            .iter()
            .map(|(name, member)| 2 * size_of::<Value>() + name.len() + value_bytes(member))
            .sum(),
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

/// What a map entry with `String` keys costs beside the bytes of its key and value.
const MAP_ENTRY: usize = 64;

fn frame_set_bytes(set: &FrameSet) -> usize {
    let text = |t: &Option<String>| t.as_ref().map_or(0, String::len);
    let frames: usize = set
        .frames
        .iter()
        .map(|frame| {
            let fields: usize = frame
                .fields
                .iter()
                .map(|field| {
                    let labels: usize = field
                        .labels
                        .iter()
                        .map(|(k, v)| MAP_ENTRY + k.len() + v.len())
                        .sum();
                    let values: usize = field
                        .values
                        .iter()
                        .map(|v| size_of::<Value>() + value_bytes(v))
                        .sum();
                    size_of_val(field)
                        + field.name.len()
                        + text(&field.unit)
                        + text(&field.display_name)
                        + labels
                        + values
                })
                .sum();
            let custom: usize = frame
                .meta
                .custom
                .iter()
                .map(|(k, v)| MAP_ENTRY + size_of::<Value>() + k.len() + value_bytes(v))
                .sum();
            let notices: usize = frame
                .meta
                .notices
                .iter()
                .map(|notice| size_of_val(notice) + notice.text.len())
                .sum();
            size_of_val(frame)
                + frame.name.len()
                + text(&frame.ref_id)
                + text(&frame.meta.executed_query_string)
                + text(&frame.meta.preferred_visualisation_type)
                + fields
                + custom
                + notices
        })
        .sum();
    size_of_val(set) + set.ref_id.len() + frames
}

/// How many (organization, instance, datasource) counters are kept; past it a datasource is
/// served from the cache and not counted.
const MAX_COUNTERS: usize = 10_000;

/// How often an idle process gives expired entries back.
const SWEEP_EVERY: Duration = Duration::from_secs(30);

type Counters = BTreeMap<(OrgId, InstanceId, String), (u64, u64)>;

/// Thread-safe query cache of the process (R3, R7, R10).
#[derive(Debug)]
pub struct QueryCache {
    cache: BoundedCache<Key, Arc<Vec<FrameSet>>>,
    counters: Mutex<Counters>, // (org, instance, datasource_uid) -> (hits, misses)
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new(64 * 1024 * 1024)
    }
}

impl QueryCache {
    /// A cache of `max_bytes` for the whole process.
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        Self {
            cache: BoundedCache::new(max_bytes.max(1024)),
            counters: Mutex::new(Counters::new()),
        }
    }

    /// Sweeps the cache on a timer for as long as it lives. Without a runtime there is no timer
    /// and the entries leave on the next use.
    pub fn sweep_on_timer(self: &Arc<Self>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let cache = Arc::downgrade(self);
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(SWEEP_EVERY).await;
                let Some(cache) = cache.upgrade() else {
                    return;
                };
                cache.sweep();
            }
        });
    }

    /// Drops what has expired, and the counters of every organization left without an entry.
    pub fn sweep(&self) {
        self.cache.sweep();
        self.counters
            .lock()
            .retain(|(org, _, _), _| self.cache.holds(*org));
    }

    /// How many entries the cache holds, expired ones included until they are swept.
    #[must_use]
    pub fn entries_held(&self) -> usize {
        self.cache.len()
    }

    /// How many bytes the cache holds.
    #[must_use]
    pub fn bytes_held(&self) -> usize {
        self.cache.bytes()
    }

    fn lookup(&self, key: &Key) -> Option<Vec<FrameSet>> {
        let found = self.cache.get(key);
        {
            let mut counters = self.counters.lock();
            let counted = (key.org_id(), key.instance_id(), key.source());
            if counters.len() < MAX_COUNTERS || counters.contains_key(&counted) {
                let (hits, misses) = counters.entry(counted).or_default();
                *(if found.is_some() { hits } else { misses }) += 1;
            }
        }
        found.map(|value| value.as_ref().clone())
    }

    fn store(&self, key: Key, value: Vec<FrameSet>, ttl: Duration) {
        let bytes = value.iter().map(frame_set_bytes).sum();
        self.cache.insert(key, Arc::new(value), bytes, ttl);
    }

    /// Looks up a cached result for `key`.
    #[must_use]
    pub fn get(&self, key: &CacheKey) -> Option<Vec<FrameSet>> {
        self.lookup(&Key::Query(key.clone()))
    }

    /// Inserts `value` for `key` with `ttl`.
    pub fn insert(&self, key: CacheKey, value: Vec<FrameSet>, ttl: Duration) {
        self.store(Key::Query(key), value, ttl);
    }

    /// Looks up a public dashboard cached result (R7).
    #[must_use]
    pub fn get_public(&self, key: &PublicCacheKey) -> Option<Vec<FrameSet>> {
        self.lookup(&Key::Public(key.clone()))
    }

    /// Inserts a public dashboard cached result (R7).
    pub fn insert_public(&self, key: PublicCacheKey, value: Vec<FrameSet>, ttl: Duration) {
        self.store(Key::Public(key), value, ttl);
    }

    /// Returns per-datasource cache statistics for `org_id` (`QueryCacheStats` entity set, R10).
    #[must_use]
    pub fn stats_for_org(
        &self,
        org_id: OrgId,
        instance_id: InstanceId,
    ) -> Vec<QueryCacheStatRecord> {
        let mut held: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for (key, bytes) in self.cache.entries_of(org_id) {
            if key.instance_id() == instance_id {
                let (entries, total) = held.entry(key.source()).or_default();
                *entries += 1;
                *total += bytes as u64;
            }
        }
        self.counters
            .lock()
            .range((org_id, instance_id, String::new())..)
            .take_while(|((org, instance, _), _)| *org == org_id && *instance == instance_id)
            .map(|((_, _, ds_uid), (hits, misses))| {
                let (entries, bytes) = held.get(ds_uid).copied().unwrap_or_default();
                QueryCacheStatRecord {
                    id: format!("{org_id}:{ds_uid}"),
                    org_id,
                    instance_id,
                    datasource_uid: ds_uid.clone(),
                    hits: *hits,
                    misses: *misses,
                    entries,
                    bytes_used: bytes,
                }
            })
            .collect()
    }
}
