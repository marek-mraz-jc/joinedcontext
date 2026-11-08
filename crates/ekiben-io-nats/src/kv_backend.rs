//! One NATS Key-Value bucket for the `nats_kv` cache, processor and output: the in-process
//! cluster for `nats://memory` URLs (tests), a JetStream KV store on a real server otherwise.

use async_nats::jetstream::kv;
use bytes::Bytes;
use ekiben_core::{CoreError, OrgContext};
use futures::TryStreamExt;
use tokio::sync::OnceCell;

use crate::broker_mem::{InMemoryNatsCluster, KvEntry};
use crate::connection::{subject_matches, NatsConnectionDetails};

pub struct KvBucket {
    conn: NatsConnectionDetails,
    bucket: String,
    store: OnceCell<kv::Store>,
}

enum Target<'a> {
    Memory(std::sync::Arc<InMemoryNatsCluster>, String),
    Live(&'a kv::Store),
}

fn op_name(op: kv::Operation) -> &'static str {
    match op {
        kv::Operation::Put => "KeyValuePutOp",
        kv::Operation::Delete => "KeyValueDeleteOp",
        kv::Operation::Purge => "KeyValuePurgeOp",
    }
}

fn entry(e: kv::Entry) -> KvEntry {
    KvEntry {
        bucket: e.bucket,
        key: e.key,
        value: e.value,
        revision: e.revision,
        delta: e.delta,
        operation: op_name(e.operation),
        created: go_rfc3339_nano(chrono::DateTime::from_timestamp_nanos(
            i64::try_from(e.created.unix_timestamp_nanos()).unwrap_or(i64::MAX),
        )),
    }
}

/// A time as Go's `time.RFC3339Nano` prints it (Bento's `created`): UTC `Z`, trailing zeros of
/// the fraction dropped.
pub fn go_rfc3339_nano(t: chrono::DateTime<chrono::Utc>) -> String {
    let base = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let frac = format!("{:09}", t.timestamp_subsec_nanos());
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{base}Z")
    } else {
        format!("{base}.{frac}Z")
    }
}

/// The text Bento's nats_kv processor flags a failed call with: a JetStream API error prints
/// as nats.go's `APIError` does.
fn live_err(e: impl std::error::Error + 'static) -> CoreError {
    CoreError::other(go_text(&e).unwrap_or_else(|| format!("nats: {e}")))
}

fn go_text(e: &(dyn std::error::Error + 'static)) -> Option<String> {
    let mut cur = Some(e);
    while let Some(err) = cur {
        if let Some(api) = err.downcast_ref::<async_nats::jetstream::Error>() {
            let v = serde_json::to_value(api).ok()?;
            return Some(format!(
                "nats: nats: API error: code={} err_code={} description={}",
                v["code"],
                v["err_code"],
                v["description"].as_str().unwrap_or_default()
            ));
        }
        cur = err.source();
    }
    None
}

impl KvBucket {
    /// `bucket` as written; an organization with a NATS subject prefix may only name buckets
    /// under that prefix (see `OrgContext::validate_nats_bucket`).
    pub fn new(
        conn: NatsConnectionDetails,
        org: &OrgContext,
        bucket: &str,
    ) -> Result<Self, CoreError> {
        org.validate_nats_bucket(bucket)?;
        let bucket = if conn.is_in_memory() {
            format!("{}::{bucket}", org.org_id.as_str())
        } else {
            bucket.to_string()
        };
        Ok(Self {
            conn,
            bucket,
            store: OnceCell::new(),
        })
    }

    async fn target(&self) -> Result<Target<'_>, CoreError> {
        if self.conn.is_in_memory() {
            let cluster = InMemoryNatsCluster::get(&self.conn.cluster_name());
            return Ok(Target::Memory(cluster, self.bucket.clone()));
        }
        let store = self
            .store
            .get_or_try_init(|| async {
                let client = self.conn.connect_live("nats_kv").await?;
                async_nats::jetstream::new(client)
                    .get_key_value(&self.bucket)
                    .await
                    .map_err(|e| {
                        CoreError::other(format!(
                            "failed to open NATS KV bucket '{}': {e}",
                            self.bucket
                        ))
                    })
            })
            .await?;
        Ok(Target::Live(store))
    }

    pub async fn get(&self, key: &str) -> Result<KvEntry, CoreError> {
        match self.target().await? {
            Target::Memory(c, b) => c.kv_get(&b, key),
            Target::Live(s) => match s.entry(key).await.map_err(live_err)? {
                Some(e) if e.operation == kv::Operation::Put => Ok(entry(e)),
                _ => Err(CoreError::KeyNotFound(key.to_string())),
            },
        }
    }

    pub async fn get_revision(&self, key: &str, revision: u64) -> Result<KvEntry, CoreError> {
        match self.target().await? {
            Target::Memory(c, b) => c.kv_get_revision(&b, key, revision),
            Target::Live(s) => match s
                .entry_for_revision(key, revision)
                .await
                .map_err(live_err)?
            {
                Some(e) => Ok(entry(e)),
                None => Err(CoreError::KeyNotFound(format!("{key}@{revision}"))),
            },
        }
    }

    pub async fn put(&self, key: &str, value: Bytes) -> Result<u64, CoreError> {
        match self.target().await? {
            Target::Memory(c, b) => Ok(c.kv_put(&b, key, value)),
            Target::Live(s) => s.put(key, value).await.map_err(live_err),
        }
    }

    pub async fn create(&self, key: &str, value: Bytes) -> Result<u64, CoreError> {
        match self.target().await? {
            Target::Memory(c, b) => c.kv_create(&b, key, value),
            Target::Live(s) => match s.create(key, value).await {
                Ok(rev) => Ok(rev),
                Err(e) if e.kind() == kv::CreateErrorKind::AlreadyExists => {
                    // The server refused the write at the key's last revision; nats.go prints
                    // that API error with the KeyExists message. The cache reads the kind.
                    let rev = s
                        .entry(key)
                        .await
                        .ok()
                        .flatten()
                        .map_or(0, |entry| entry.revision);
                    Err(CoreError::KeyAlreadyExists(format!(
                        "nats: nats: API error: code=400 err_code=10071 description=wrong last sequence: {rev}: key exists"
                    )))
                }
                Err(e) => Err(live_err(e)),
            },
        }
    }

    pub async fn update(&self, key: &str, value: Bytes, revision: u64) -> Result<u64, CoreError> {
        match self.target().await? {
            Target::Memory(c, b) => c.kv_update(&b, key, value, revision),
            Target::Live(s) => s.update(key, value, revision).await.map_err(live_err),
        }
    }

    /// Deletes `key` and returns the revision the delete marker took (0 on a live server, which
    /// does not report it).
    pub async fn delete(&self, key: &str) -> Result<u64, CoreError> {
        match self.target().await? {
            Target::Memory(c, b) => Ok(c.kv_delete(&b, key)),
            Target::Live(s) => s.delete(key).await.map(|()| 0).map_err(live_err),
        }
    }

    pub async fn purge(&self, key: &str) -> Result<u64, CoreError> {
        match self.target().await? {
            Target::Memory(c, b) => Ok(c.kv_purge(&b, key)),
            Target::Live(s) => s.purge(key).await.map(|()| 0).map_err(live_err),
        }
    }

    pub async fn history(&self, key: &str) -> Result<Vec<KvEntry>, CoreError> {
        match self.target().await? {
            Target::Memory(c, b) => Ok(c.kv_history(&b, key)),
            Target::Live(s) => {
                let h = s.history(key).await.map_err(live_err)?;
                h.map_ok(entry).try_collect().await.map_err(live_err)
            }
        }
    }

    pub async fn keys(&self, filter: &str) -> Result<Vec<String>, CoreError> {
        match self.target().await? {
            Target::Memory(c, b) => Ok(c.kv_keys(&b, filter)),
            Target::Live(s) => {
                let keys: Vec<String> = s
                    .keys()
                    .await
                    .map_err(live_err)?
                    .try_collect()
                    .await
                    .map_err(live_err)?;
                Ok(keys
                    .into_iter()
                    .filter(|k| subject_matches(filter, k))
                    .collect())
            }
        }
    }
}
