use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_core::config::parse_duration_str;
use ekiben_core::{Cache, ComponentConfig, CoreError, Part, RateLimit, Registry, StreamContext};
use serde_json::Value as JsonValue;

pub fn register(registry: &Registry) {
    registry.register_rate_limit("local", |conf, _ctx| {
        Ok(Arc::new(LocalRateLimit::from_config(conf)?))
    });

    for name in ["memory", "lru", "ttlru"] {
        registry.register_cache(name, |conf, _ctx| {
            Ok(Arc::new(MemoryCache::from_config(conf)?))
        });
    }

    registry.register_cache("noop", |_conf, _ctx| Ok(Arc::new(NoopCache)));

    registry.register_cache("multilevel", |conf, ctx| {
        Ok(Arc::new(MultiLevelCache::from_config(conf, ctx)?))
    });
}

struct RateLimitState {
    bucket: isize,
    byte_bucket: isize,
    exceeded_limit: bool,
    last_refresh: Instant,
}

/// Implements `local` rate limit (`internal/impl/pure/rate_limit_local.go`).
pub struct LocalRateLimit {
    size: isize,
    byte_size: isize,
    period: Duration,
    state: Mutex<RateLimitState>,
}

impl LocalRateLimit {
    pub fn new(count: isize, byte_size: isize, interval: Duration) -> Result<Self, CoreError> {
        if count < 0 || byte_size < 0 {
            return Err(CoreError::config(
                "neither byte size nor count can be negative",
            ));
        }
        if count == 0 && byte_size == 0 {
            return Err(CoreError::config(
                "either count or byte size must be larger than zero",
            ));
        }
        Ok(Self {
            size: count,
            byte_size,
            period: interval,
            state: Mutex::new(RateLimitState {
                bucket: count,
                byte_bucket: byte_size,
                exceeded_limit: false,
                last_refresh: Instant::now(),
            }),
        })
    }

    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let count = conf
            .config
            .get("count")
            .and_then(JsonValue::as_i64)
            .unwrap_or(1000) as isize;
        let byte_size = conf
            .config
            .get("byte_size")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0) as isize;
        let interval_str = conf
            .config
            .get("interval")
            .and_then(JsonValue::as_str)
            .unwrap_or("1s");
        let interval = parse_duration_str(interval_str)?;
        Self::new(count, byte_size, interval)
    }

    fn refresh(st: &mut RateLimitState, size: isize, byte_size: isize) {
        st.bucket = size;
        st.byte_bucket = byte_size;
        st.last_refresh = Instant::now();
        st.exceeded_limit = false;
    }
}

#[async_trait]
impl RateLimit for LocalRateLimit {
    async fn access(&self, parts: &[Part]) -> Result<Duration, CoreError> {
        let mut st = self.state.lock().unwrap();
        if st.last_refresh.elapsed() >= self.period {
            Self::refresh(&mut st, self.size, self.byte_size);
        }

        if !st.exceeded_limit && self.byte_size > 0 {
            for p in parts {
                st.byte_bucket -= p.as_bytes_ref().len() as isize;
                if st.byte_bucket < 0 {
                    st.exceeded_limit = true;
                    break;
                }
            }
        }

        if self.size > 0 {
            st.bucket -= 1;
            if st.bucket < 0 {
                st.exceeded_limit = true;
            }
        }

        if !st.exceeded_limit {
            return Ok(Duration::ZERO);
        }

        let elapsed = st.last_refresh.elapsed();
        if elapsed < self.period {
            return Ok(self.period - elapsed);
        }

        Self::refresh(&mut st, self.size, self.byte_size);
        Ok(Duration::ZERO)
    }
}

struct CacheEntry {
    value: Bytes,
    expires_at: Option<Instant>,
    /// When the key was last read or written, on `Entries::clock`.
    used: u64,
}

/// The keys with their last use, so a capped cache (`lru`, `ttlru`) evicts the least recently
/// used key, as Bento's do, rather than an arbitrary map key.
#[derive(Default)]
struct Entries {
    map: HashMap<String, CacheEntry>,
    by_use: BTreeMap<u64, String>,
    clock: u64,
}

impl Entries {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn touch(&mut self, key: &str) {
        let now = self.tick();
        if let Some(e) = self.map.get_mut(key) {
            self.by_use.remove(&e.used);
            e.used = now;
            self.by_use.insert(now, key.to_string());
        }
    }

    fn insert(&mut self, key: &str, value: Bytes, expires_at: Option<Instant>, cap: Option<usize>) {
        if let Some(cap) = cap {
            while !self.map.contains_key(key) && self.map.len() >= cap.max(1) {
                let Some((_, oldest)) = self.by_use.pop_first() else {
                    break;
                };
                self.map.remove(&oldest);
            }
        }
        let used = self.tick();
        let entry = CacheEntry {
            value,
            expires_at,
            used,
        };
        if let Some(old) = self.map.insert(key.to_string(), entry) {
            self.by_use.remove(&old.used);
        }
        self.by_use.insert(used, key.to_string());
    }

    fn remove(&mut self, key: &str) {
        if let Some(e) = self.map.remove(key) {
            self.by_use.remove(&e.used);
        }
    }
}

/// Implements `memory` / `lru` / `ttlru` cache (`internal/impl/pure/cache_memory.go`,
/// `cache_lru.go`, `cache_ttlru.go`). A `cap` evicts the least recently used key.
/// ponytail: `lru`/`ttlru` keep no cap unless one is configured (Bento defaults to 1000/1024),
/// so live pipelines keep every key they kept before.
pub struct MemoryCache {
    default_ttl: Option<Duration>,
    cap: Option<usize>,
    entries: Mutex<Entries>,
}

impl MemoryCache {
    pub fn new(default_ttl: Option<Duration>, cap: Option<usize>) -> Self {
        Self {
            default_ttl,
            cap,
            entries: Mutex::new(Entries::default()),
        }
    }

    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let default_ttl = match conf
            .config
            .get("default_ttl")
            .or_else(|| conf.config.get("ttl"))
            .and_then(JsonValue::as_str)
        {
            Some(s) if !s.is_empty() => {
                let d = parse_duration_str(s)?;
                (!d.is_zero()).then_some(d)
            }
            _ => None,
        };

        let cap = conf
            .config
            .get("cap")
            .and_then(JsonValue::as_u64)
            .map(|c| c as usize);

        let mut entries = Entries::default();
        if let Some(init_map) = conf
            .config
            .get("init_values")
            .and_then(JsonValue::as_object)
        {
            for (k, v) in init_map {
                let s = v.as_str().unwrap_or("").to_string();
                entries.insert(
                    k,
                    Bytes::from(s.into_bytes()),
                    default_ttl.map(|d| Instant::now() + d),
                    cap,
                );
            }
        }

        Ok(Self {
            default_ttl,
            cap,
            entries: Mutex::new(entries),
        })
    }

    fn effective_expiry(&self, ttl: Option<Duration>) -> Option<Instant> {
        ttl.or(self.default_ttl).map(|d| Instant::now() + d)
    }

    fn lock(&self) -> MutexGuard<'_, Entries> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl Cache for MemoryCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        let mut guard = self.lock();
        let Some(entry) = guard.map.get(key) else {
            return Err(CoreError::KeyNotFound(key.to_string()));
        };
        if entry.expires_at.is_some_and(|exp| Instant::now() >= exp) {
            guard.remove(key);
            return Err(CoreError::KeyNotFound(key.to_string()));
        }
        let value = entry.value.clone();
        guard.touch(key);
        Ok(value)
    }

    async fn set(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        let expires_at = self.effective_expiry(ttl);
        self.lock().insert(key, value, expires_at, self.cap);
        Ok(())
    }

    async fn add(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        let mut guard = self.lock();
        if let Some(entry) = guard.map.get(key) {
            if entry.expires_at.is_none_or(|exp| Instant::now() < exp) {
                return Err(CoreError::KeyAlreadyExists(key.to_string()));
            }
        }
        let expires_at = self.effective_expiry(ttl);
        guard.insert(key, value, expires_at, self.cap);
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CoreError> {
        self.lock().remove(key);
        Ok(())
    }
}

pub struct NoopCache;

#[async_trait]
impl Cache for NoopCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        Err(CoreError::KeyNotFound(key.to_string()))
    }
    async fn set(
        &self,
        _key: &str,
        _value: Bytes,
        _ttl: Option<Duration>,
    ) -> Result<(), CoreError> {
        Ok(())
    }
    async fn add(
        &self,
        _key: &str,
        _value: Bytes,
        _ttl: Option<Duration>,
    ) -> Result<(), CoreError> {
        Ok(())
    }
    async fn delete(&self, _key: &str) -> Result<(), CoreError> {
        Ok(())
    }
}

pub struct MultiLevelCache {
    tiers: Vec<Arc<dyn Cache>>,
}

impl MultiLevelCache {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let arr = conf
            .config
            .as_array()
            .ok_or_else(|| CoreError::config("multilevel cache requires an array of caches"))?;
        let mut tiers = Vec::with_capacity(arr.len());
        for item in arr {
            if let Some(res_name) = item.as_str() {
                tiers.push(ctx.resources.get_cache(ctx.org_id(), res_name)?);
            } else {
                let c = ComponentConfig::from_json_value(item, "memory", None)?;
                tiers.push(ctx.registry.build_cache(&c, ctx)?);
            }
        }
        Ok(Self { tiers })
    }
}

#[async_trait]
impl Cache for MultiLevelCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        for (i, tier) in self.tiers.iter().enumerate() {
            if let Ok(val) = tier.get(key).await {
                for earlier in &self.tiers[..i] {
                    let _ = earlier.set(key, val.clone(), None).await;
                }
                return Ok(val);
            }
        }
        Err(CoreError::KeyNotFound(key.to_string()))
    }

    async fn set(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        for tier in &self.tiers {
            tier.set(key, value.clone(), ttl).await?;
        }
        Ok(())
    }

    async fn add(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        for tier in &self.tiers {
            tier.add(key, value.clone(), ttl).await?;
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CoreError> {
        for tier in &self.tiers {
            tier.delete(key).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &'static str) -> Bytes {
        Bytes::from_static(s.as_bytes())
    }

    #[tokio::test]
    async fn a_capped_cache_evicts_the_least_recently_used_key() {
        let c = MemoryCache::new(None, Some(2));
        c.set("a", b("1"), None).await.unwrap();
        c.set("b", b("2"), None).await.unwrap();
        c.get("a").await.unwrap();
        c.set("c", b("3"), None).await.unwrap();
        assert!(matches!(c.get("b").await, Err(CoreError::KeyNotFound(_))));
        assert_eq!(c.get("a").await.unwrap(), b("1"));
        // Overwriting a held key evicts nothing.
        c.set("a", b("4"), None).await.unwrap();
        assert_eq!(c.get("c").await.unwrap(), b("3"));
        assert_eq!(c.get("a").await.unwrap(), b("4"));
    }

    #[tokio::test]
    async fn an_uncapped_cache_keeps_every_key_and_expired_ones_go() {
        let c = MemoryCache::new(None, None);
        for k in ["a", "b", "c", "d"] {
            c.set(k, b("v"), None).await.unwrap();
        }
        for k in ["a", "b", "c", "d"] {
            assert!(c.get(k).await.is_ok(), "{k}");
        }
        c.set("t", b("v"), Some(Duration::from_nanos(1)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        assert!(c.get("t").await.is_err());
        c.add("t", b("w"), None).await.unwrap();
        assert!(c.add("t", b("x"), None).await.is_err());
    }
}
