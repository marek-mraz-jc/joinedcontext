use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_core::{Cache, ComponentConfig, CoreError, Registry, StreamContext};
use serde_json::Value as JsonValue;

use crate::connection::NatsConnectionDetails;
use crate::kv_backend::KvBucket;

pub fn register(registry: &Registry) {
    registry.register_cache("nats_kv", |conf, ctx| {
        Ok(Arc::new(NatsKvCache::from_config(conf, ctx)?))
    });
}

/// Implements `nats_kv` cache (`internal/impl/nats/cache_kv.go`) over [`KvBucket`].
pub struct NatsKvCache {
    bucket: KvBucket,
}

impl NatsKvCache {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let conn_details = NatsConnectionDetails::from_json(&conf.config)?;
        let bucket = conf
            .config
            .get("bucket")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::config("nats_kv cache requires a 'bucket' field"))?;
        Ok(Self {
            bucket: KvBucket::new(conn_details, &ctx.org, bucket)?,
        })
    }
}

#[async_trait]
impl Cache for NatsKvCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        Ok(self.bucket.get(key).await?.value)
    }

    async fn exists(&self, key: &str) -> Result<bool, CoreError> {
        match self.get(key).await {
            Ok(_) => Ok(true),
            Err(CoreError::KeyNotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn set(&self, key: &str, value: Bytes, _ttl: Option<Duration>) -> Result<(), CoreError> {
        self.bucket.put(key, value).await.map(|_| ())
    }

    async fn add(&self, key: &str, value: Bytes, _ttl: Option<Duration>) -> Result<(), CoreError> {
        self.bucket.create(key, value).await.map(|_| ())
    }

    async fn delete(&self, key: &str) -> Result<(), CoreError> {
        self.bucket.delete(key).await.map(|_| ())
    }
}
