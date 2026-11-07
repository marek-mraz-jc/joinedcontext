#![forbid(unsafe_code)]
//! Per-organization query cache (R3, R7, §2.6, §4.3 CVE-2022-23498, §5.3 test 5):
//! - Keyed by `(org_id, instance_id, subject_scope_hash, datasource_uid, canonical_query_hash, aligned_from_ms, aligned_to_ms, interval_ms)`.
//! - Public dashboard cache key: `(org_id, instance_id, dashboard_uid, dashboard_version, panel_id, from_bucket_ms, to_bucket_ms)`.
//! - Never shares cache entries across organizations or across different subject permission scopes!
//! - Enforces per-organization byte ceiling (`max_cache_bytes`) with FIFO/LRU eviction and tracks
//!   per-datasource hit/miss/byte statistics (`QueryCacheStatRecord`).

use controlroom_model::{
    CanonicalJson, FrameSet, InstanceId, OrgId, QueryCacheStatRecord, sha256_hex,
};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// Cache key for authenticated datasource queries (§2.6, §4.3 CVE-2022-23498).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheKey {
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub subject_scope_hash: String,
    pub datasource_uid: String,
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
            canonical_query_hash,
            aligned_from_ms,
            aligned_to_ms,
            interval_ms,
        }
    }
}

/// Cache key for anonymous public dashboard panel queries (R7: per `(org_id, dashboard_uid, dashboard_version, panel_id, range_bucket)`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicCacheKey {
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub dashboard_uid: String,
    pub dashboard_version: i32,
    pub panel_id: u32,
    pub from_bucket_ms: i64,
    pub to_bucket_ms: i64,
}

impl PublicCacheKey {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        org_id: OrgId,
        instance_id: InstanceId,
        dashboard_uid: impl Into<String>,
        dashboard_version: i32,
        panel_id: u32,
        from_ms: i64,
        to_ms: i64,
        bucket_ms: i64,
    ) -> Self {
        let b = bucket_ms.max(1_000);
        Self {
            org_id,
            instance_id,
            dashboard_uid: dashboard_uid.into(),
            dashboard_version,
            panel_id,
            from_bucket_ms: from_ms.div_euclid(b) * b,
            to_bucket_ms: to_ms.div_euclid(b) * b,
        }
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    value: Vec<FrameSet>,
    expires_at: Instant,
    bytes: usize,
    datasource_uid: String,
}

#[derive(Debug, Default)]
struct OrgCacheBucket {
    entries: BTreeMap<CacheKey, CacheEntry>,
    order: VecDeque<CacheKey>,
    public_entries: BTreeMap<PublicCacheKey, CacheEntry>,
    public_order: VecDeque<PublicCacheKey>,
    bytes_used: usize,
    stats: BTreeMap<String, (u64, u64)>, // datasource_uid -> (hits, misses)
}

/// Thread-safe per-organization query cache (R3, R7, R10).
#[derive(Debug)]
pub struct QueryCache {
    max_bytes_per_org: usize,
    orgs: Mutex<BTreeMap<(OrgId, InstanceId), OrgCacheBucket>>,
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new(64 * 1024 * 1024)
    }
}

impl QueryCache {
    #[must_use]
    pub fn new(max_bytes_per_org: usize) -> Self {
        Self {
            max_bytes_per_org: max_bytes_per_org.max(1024),
            orgs: Mutex::new(BTreeMap::new()),
        }
    }

    /// Looks up a cached result for `key` within `key.org_id`'s isolated bucket.
    #[must_use]
    pub fn get(&self, key: &CacheKey) -> Option<Vec<FrameSet>> {
        let mut guard = self.orgs.lock();
        let bucket = guard.entry((key.org_id, key.instance_id)).or_default();
        let now = Instant::now();
        if let Some(entry) = bucket.entries.get(key)
            && entry.expires_at > now
        {
            let res = entry.value.clone();
            let stat = bucket.stats.entry(key.datasource_uid.clone()).or_default();
            stat.0 += 1;
            return Some(res);
        }
        if let Some(expired) = bucket.entries.remove(key) {
            bucket.bytes_used = bucket.bytes_used.saturating_sub(expired.bytes);
        }
        let stat = bucket.stats.entry(key.datasource_uid.clone()).or_default();
        stat.1 += 1;
        None
    }

    /// Inserts `value` for `key` with `ttl`, evicting older entries in the same organization if needed.
    pub fn insert(&self, key: CacheKey, value: Vec<FrameSet>, ttl: Duration) {
        let approx_bytes = serde_json::to_vec(&value).map_or(256, |b| b.len());
        if approx_bytes > self.max_bytes_per_org {
            return;
        }
        let mut guard = self.orgs.lock();
        let bucket = guard.entry((key.org_id, key.instance_id)).or_default();

        if let Some(old) = bucket.entries.remove(&key) {
            bucket.bytes_used = bucket.bytes_used.saturating_sub(old.bytes);
        }

        while bucket.bytes_used + approx_bytes > self.max_bytes_per_org {
            let Some(oldest_key) = bucket.order.pop_front() else {
                break;
            };
            if let Some(evicted) = bucket.entries.remove(&oldest_key) {
                bucket.bytes_used = bucket.bytes_used.saturating_sub(evicted.bytes);
            }
        }

        bucket.bytes_used += approx_bytes;
        bucket.order.push_back(key.clone());
        bucket.entries.insert(
            key.clone(),
            CacheEntry {
                value,
                expires_at: Instant::now() + ttl,
                bytes: approx_bytes,
                datasource_uid: key.datasource_uid,
            },
        );
    }

    /// Looks up a public dashboard cached result (R7).
    #[must_use]
    pub fn get_public(&self, key: &PublicCacheKey) -> Option<Vec<FrameSet>> {
        let mut guard = self.orgs.lock();
        let bucket = guard.entry((key.org_id, key.instance_id)).or_default();
        let now = Instant::now();
        if let Some(entry) = bucket.public_entries.get(key)
            && entry.expires_at > now
        {
            let res = entry.value.clone();
            let stat = bucket
                .stats
                .entry(format!("public:{}", key.dashboard_uid))
                .or_default();
            stat.0 += 1;
            return Some(res);
        }
        if let Some(expired) = bucket.public_entries.remove(key) {
            bucket.bytes_used = bucket.bytes_used.saturating_sub(expired.bytes);
        }
        let stat = bucket
            .stats
            .entry(format!("public:{}", key.dashboard_uid))
            .or_default();
        stat.1 += 1;
        None
    }

    /// Inserts a public dashboard cached result (R7).
    pub fn insert_public(&self, key: PublicCacheKey, value: Vec<FrameSet>, ttl: Duration) {
        let approx_bytes = serde_json::to_vec(&value).map_or(256, |b| b.len());
        if approx_bytes > self.max_bytes_per_org {
            return;
        }
        let mut guard = self.orgs.lock();
        let bucket = guard.entry((key.org_id, key.instance_id)).or_default();
        if let Some(old) = bucket.public_entries.remove(&key) {
            bucket.bytes_used = bucket.bytes_used.saturating_sub(old.bytes);
        }
        while bucket.bytes_used + approx_bytes > self.max_bytes_per_org {
            let Some(oldest) = bucket.public_order.pop_front() else {
                break;
            };
            if let Some(ev) = bucket.public_entries.remove(&oldest) {
                bucket.bytes_used = bucket.bytes_used.saturating_sub(ev.bytes);
            }
        }
        bucket.bytes_used += approx_bytes;
        let ds_tag = format!("public:{}", key.dashboard_uid);
        bucket.public_order.push_back(key.clone());
        bucket.public_entries.insert(
            key,
            CacheEntry {
                value,
                expires_at: Instant::now() + ttl,
                bytes: approx_bytes,
                datasource_uid: ds_tag,
            },
        );
    }

    /// Returns per-datasource cache statistics for `org_id` (`QueryCacheStats` entity set, R10).
    #[must_use]
    pub fn stats_for_org(
        &self,
        org_id: OrgId,
        instance_id: InstanceId,
    ) -> Vec<QueryCacheStatRecord> {
        let guard = self.orgs.lock();
        let Some(bucket) = guard.get(&(org_id, instance_id)) else {
            return Vec::new();
        };
        bucket
            .stats
            .iter()
            .map(|(ds_uid, (hits, misses))| {
                let (entries, bytes) = bucket
                    .entries
                    .values()
                    .chain(bucket.public_entries.values())
                    .filter(|e| &e.datasource_uid == ds_uid)
                    .fold((0u64, 0u64), |(c, b), e| (c + 1, b + e.bytes as u64));
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
