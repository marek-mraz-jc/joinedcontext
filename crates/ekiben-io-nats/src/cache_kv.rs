use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_core::{Cache, ComponentConfig, CoreError, OrgId, Registry, StreamContext};
use serde_json::Value as JsonValue;

use crate::broker_mem::InMemoryNatsCluster;
use crate::connection::NatsConnectionDetails;

pub fn register(registry: &Registry) {
    registry.register_cache("nats_kv", |conf, ctx| {
        Ok(Arc::new(NatsKvCache::from_config(conf, ctx)?))
    });
}

/// Implements `nats_kv` cache (`internal/impl/nats/cache_kv.go`).
/// Bucket names are automatically namespaced by `OrgId` when running on a shared cluster.
pub struct NatsKvCache {
    conn_details: NatsConnectionDetails,
    effective_bucket: String,
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
        Ok(Self::new(conn_details, ctx.org_id(), bucket))
    }

    pub fn new(conn_details: NatsConnectionDetails, org_id: &OrgId, bucket: &str) -> Self {
        Self {
            conn_details,
            effective_bucket: format!("{}::{}", org_id.as_str(), bucket),
        }
    }
}

#[async_trait]
impl Cache for NatsKvCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        let cluster = InMemoryNatsCluster::get(&self.conn_details.cluster_name());
        let entry = cluster.kv_get(&self.effective_bucket, key)?;
        Ok(entry.value)
    }

    async fn exists(&self, key: &str) -> Result<bool, CoreError> {
        match self.get(key).await {
            Ok(_) => Ok(true),
            Err(CoreError::KeyNotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn set(&self, key: &str, value: Bytes, _ttl: Option<Duration>) -> Result<(), CoreError> {
        let cluster = InMemoryNatsCluster::get(&self.conn_details.cluster_name());
        cluster.kv_put(&self.effective_bucket, key, value);
        Ok(())
    }

    async fn add(&self, key: &str, value: Bytes, _ttl: Option<Duration>) -> Result<(), CoreError> {
        let cluster = InMemoryNatsCluster::get(&self.conn_details.cluster_name());
        cluster.kv_create(&self.effective_bucket, key, value)?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CoreError> {
        let cluster = InMemoryNatsCluster::get(&self.conn_details.cluster_name());
        cluster.kv_delete(&self.effective_bucket, key);
        Ok(())
    }
}
