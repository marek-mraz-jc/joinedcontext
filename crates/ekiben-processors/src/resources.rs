use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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
}

/// Implements `memory` / `lru` / `ttlru` cache (`internal/impl/pure/cache_memory.go`).
pub struct MemoryCache {
    default_ttl: Option<Duration>,
    cap: Option<usize>,
    entries: Mutex<HashMap<String, CacheEntry>>,
}

impl MemoryCache {
    pub fn new(default_ttl: Option<Duration>, cap: Option<usize>) -> Self {
        Self {
            default_ttl,
            cap,
            entries: Mutex::new(HashMap::new()),
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

        let mut entries = HashMap::new();
        if let Some(init_map) = conf
            .config
            .get("init_values")
            .and_then(JsonValue::as_object)
        {
            for (k, v) in init_map {
                let s = v.as_str().unwrap_or("").to_string();
                entries.insert(
                    k.clone(),
                    CacheEntry {
                        value: Bytes::from(s.into_bytes()),
                        expires_at: default_ttl.map(|d| Instant::now() + d),
                    },
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
}

#[async_trait]
impl Cache for MemoryCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        let mut guard = self.entries.lock().unwrap();
        if let Some(entry) = guard.get(key) {
            if entry.expires_at.is_some_and(|exp| Instant::now() >= exp) {
                guard.remove(key);
                return Err(CoreError::KeyNotFound(key.to_string()));
            }
            return Ok(entry.value.clone());
        }
        Err(CoreError::KeyNotFound(key.to_string()))
    }

    async fn set(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        let mut guard = self.entries.lock().unwrap();
        if let Some(cap) = self.cap {
            if guard.len() >= cap && !guard.contains_key(key) {
                if let Some(first_k) = guard.keys().next().cloned() {
                    guard.remove(&first_k);
                }
            }
        }
        guard.insert(
            key.to_string(),
            CacheEntry {
                value,
                expires_at: self.effective_expiry(ttl),
            },
        );
        Ok(())
    }

    async fn add(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        let mut guard = self.entries.lock().unwrap();
        if let Some(entry) = guard.get(key) {
            if entry.expires_at.is_none_or(|exp| Instant::now() < exp) {
                return Err(CoreError::KeyAlreadyExists(key.to_string()));
            }
        }
        guard.insert(
            key.to_string(),
            CacheEntry {
                value,
                expires_at: self.effective_expiry(ttl),
            },
        );
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CoreError> {
        self.entries.lock().unwrap().remove(key);
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
