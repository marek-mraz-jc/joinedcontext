#![forbid(unsafe_code)]
//! Per-organization / per-instance quotas (S6, R§2.6) and the fixed caps of Git work.

use crate::error::{ShakoError, ShakoResult};
use crate::model::{InstanceRecord, QuotaUsageRecord};

/// Default limits from R§2.6 and R§7.
pub const MAX_SINGLE_PUSH_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_SINGLE_OBJECT_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_REFS_PER_PUSH: usize = 1_000;
pub const MAX_REFS_PER_REPO: usize = 10_000;
pub const MAX_DELTA_CHAIN_DEPTH: usize = 50;
pub const MAX_PKT_LINE_LEN: usize = 65_520;

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
