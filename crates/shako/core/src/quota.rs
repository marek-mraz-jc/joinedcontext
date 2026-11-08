#![forbid(unsafe_code)]
//! Memory-weighted burst semaphore (192 MiB, S22) and per-organization / per-instance quotas (S6, R§2.6).

use crate::error::{ShakoError, ShakoResult};
use crate::model::{InstanceRecord, QuotaUsageRecord};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Default limits from R§2.6 and R§7.
pub const DEFAULT_MEMORY_SEMAPHORE_BYTES: u32 = 192 * 1024 * 1024;
pub const PACK_OPERATION_WEIGHT_BYTES: u32 = 16 * 1024 * 1024;
pub const MERGE_OPERATION_WEIGHT_BYTES: u32 = 8 * 1024 * 1024;
pub const ARCHIVE_OPERATION_WEIGHT_BYTES: u32 = 4 * 1024 * 1024;

pub const MAX_SINGLE_PUSH_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_SINGLE_OBJECT_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_REFS_PER_PUSH: usize = 1_000;
pub const MAX_REFS_PER_REPO: usize = 10_000;
pub const MAX_DELTA_CHAIN_DEPTH: usize = 50;
pub const MAX_PKT_LINE_LEN: usize = 65_520;

/// Memory-weighted semaphore governing concurrent pack, archive, and merge operations (S22).
/// Requests that cannot acquire their weight within `acquire_timeout` receive `503` (`ShakoError::Overloaded`).
#[derive(Clone)]
pub struct MemoryBudget {
    semaphore: Arc<Semaphore>,
    capacity_bytes: u32,
    acquire_timeout: Duration,
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self::new(DEFAULT_MEMORY_SEMAPHORE_BYTES, Duration::from_secs(2))
    }
}

impl MemoryBudget {
    #[must_use]
    pub fn new(capacity_bytes: u32, acquire_timeout: Duration) -> Self {
        let cap = capacity_bytes.max(1);
        Self {
            semaphore: Arc::new(Semaphore::new(cap as usize)),
            capacity_bytes: cap,
            acquire_timeout,
        }
    }

    #[must_use]
    pub const fn capacity_bytes(&self) -> u32 {
        self.capacity_bytes
    }

    #[must_use]
    pub fn available_bytes(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Acquires `bytes` permits from the memory budget, returning `ShakoError::Overloaded` (503) on timeout.
    pub async fn acquire(&self, bytes: u32) -> ShakoResult<OwnedSemaphorePermit> {
        let needed = bytes.clamp(1, self.capacity_bytes);
        match tokio::time::timeout(
            self.acquire_timeout,
            self.semaphore.clone().acquire_many_owned(needed),
        )
        .await
        {
            Ok(Ok(permit)) => Ok(permit),
            _ => Err(ShakoError::Overloaded(
                "memory semaphore saturated; retry after 5 seconds".into(),
            )),
        }
    }

    /// Acquires `bytes` immediately without waiting, returning `503` if unavailable.
    pub fn try_acquire(&self, bytes: u32) -> ShakoResult<OwnedSemaphorePermit> {
        let needed = bytes.clamp(1, self.capacity_bytes);
        self.semaphore
            .clone()
            .try_acquire_many_owned(needed)
            .map_err(|_| {
                ShakoError::Overloaded("memory semaphore saturated; retry after 5 seconds".into())
            })
    }
}

/// Checks whether creating another repository is within the instance/organization quota (S6, R§2.6).
pub fn check_repo_count_quota(
    instance: &InstanceRecord,
    usage: &QuotaUsageRecord,
) -> ShakoResult<()> {
    if usage.repo_count >= instance.max_repos {
        return Err(ShakoError::QuotaExceeded {
            resource: "repositories".into(),
            limit: instance.max_repos,
        });
    }
    Ok(())
}

/// Returns the maximum additional bytes a single push or package upload may stream before hitting
/// either `max_push_bytes` or the remaining organization storage quota (S6, R§2.6).
pub fn remaining_storage_allowance(
    instance: &InstanceRecord,
    usage: &QuotaUsageRecord,
) -> ShakoResult<u64> {
    if usage.total_bytes() >= instance.max_storage_bytes {
        return Err(ShakoError::QuotaExceeded {
            resource: "storage_bytes".into(),
            limit: instance.max_storage_bytes,
        });
    }
    let left = instance.max_storage_bytes - usage.total_bytes();
    Ok(left.min(instance.max_push_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_budget_returns_503_when_saturated() {
        let budget = MemoryBudget::new(16 * 1024 * 1024, Duration::from_millis(20));
        let _p1 = budget.acquire(16 * 1024 * 1024).await.expect("first permit");
        let err = budget
            .acquire(4 * 1024 * 1024)
            .await
            .expect_err("saturated");
        assert_eq!(err.status_code(), 503);
        assert_eq!(err.slug(), "shako.overloaded");
    }
}
