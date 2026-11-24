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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{InstanceId, OrgId};
    use jc_time::TimestampTz;

    fn usage(repo_count: u64, git_bytes: u64, package_bytes: u64) -> QuotaUsageRecord {
        QuotaUsageRecord {
            instance_id: InstanceId::new(),
            org_id: OrgId::new(),
            repo_count,
            git_bytes,
            package_bytes,
            updated_at: TimestampTz::now(),
        }
    }

    #[test]
    fn repo_count_quota_refuses_at_the_limit() {
        let mut instance = InstanceRecord::default_for(InstanceId::new(), OrgId::new(), "h");
        instance.max_repos = 2;
        assert!(check_repo_count_quota(&instance, &usage(1, 0, 0)).is_ok());
        assert_eq!(
            check_repo_count_quota(&instance, &usage(2, 0, 0)),
            Err(ShakoError::QuotaExceeded {
                resource: "repositories".into(),
                limit: 2
            })
        );
    }

    #[test]
    fn storage_allowance_is_the_smaller_of_push_cap_and_room_left() {
        let mut instance = InstanceRecord::default_for(InstanceId::new(), OrgId::new(), "h");
        instance.max_storage_bytes = 1_000;
        instance.max_push_bytes = 300;
        assert_eq!(
            remaining_storage_allowance(&instance, &usage(0, 100, 0)),
            Ok(300)
        );
        assert_eq!(
            remaining_storage_allowance(&instance, &usage(0, 600, 200)),
            Ok(200)
        );
        assert_eq!(
            remaining_storage_allowance(&instance, &usage(0, 900, 100)),
            Err(ShakoError::QuotaExceeded {
                resource: "storage_bytes".into(),
                limit: 1_000
            })
        );
    }
}
