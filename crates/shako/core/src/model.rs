#![forbid(unsafe_code)]
//! Persistent domain records for `shako` (S2, S5, S10, S13, S14, S15).
//! Every row carries `instance_id` and `org_id`.

use crate::ids::{InstanceId, OrgId, PackageId, RepoId, WebhookId};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_true() -> bool {
    true
}

/// Forge instance record (`shako.instance`, S2, S6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRecord {
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub host: String,
    pub path_prefix: String,
    pub max_repos: u64,
    pub max_storage_bytes: u64,
    pub max_push_bytes: u64,
    pub secret_scan_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl InstanceRecord {
    #[must_use]
    pub fn default_for(instance_id: InstanceId, org_id: OrgId, host: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            instance_id,
            org_id,
            host: host.into(),
            path_prefix: String::new(),
            max_repos: 200,
            max_storage_bytes: 5 * 1024 * 1024 * 1024,
            max_push_bytes: 512 * 1024 * 1024,
            secret_scan_enabled: true,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Repository record (`shako.repo`, S2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepoRecord {
    pub id: RepoId,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub project_id: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub default_branch: String,
    pub private: bool,
    pub archived: bool,
    pub size_bytes: u64,
    pub next_pr_number: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl RepoRecord {
    /// Deterministic positive numeric ID for Gitea `repo_id` compatibility (`gitea.rs:675-681`).
    #[must_use]
    pub fn numeric_id(&self) -> u64 {
        let bytes = self.id.0.as_bytes();
        let raw = u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        (raw & 0x7fff_ffff_ffff_ffff).max(1)
    }
}

/// Branch protection rule (`shako.protection_rule`, S5, R§1.4, R§3.3).
/// Keeps Gitea 1.27.3 field names for wire round-tripping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BranchProtectionRule {
    pub rule_name: String,
    #[serde(default = "default_true")]
    pub enable_push: bool,
    #[serde(default)]
    pub enable_push_whitelist: bool,
    #[serde(default)]
    pub push_whitelist_usernames: Vec<String>,
    #[serde(default)]
    pub push_whitelist_teams: Vec<String>,
    #[serde(default)]
    pub push_whitelist_deploy_keys: bool,
    #[serde(default)]
    pub enable_force_push: bool,
    #[serde(default)]
    pub enable_force_push_allowlist: bool,
    #[serde(default)]
    pub force_push_allowlist_usernames: Vec<String>,
    #[serde(default)]
    pub force_push_allowlist_teams: Vec<String>,
    #[serde(default)]
    pub force_push_allowlist_deploy_keys: bool,
    #[serde(default)]
    pub enable_merge_whitelist: bool,
    #[serde(default)]
    pub merge_whitelist_usernames: Vec<String>,
    #[serde(default)]
    pub merge_whitelist_teams: Vec<String>,
    #[serde(default)]
    pub enable_status_check: bool,
    #[serde(default, alias = "required_status_checks")]
    pub status_check_contexts: Vec<String>,
    #[serde(default)]
    pub required_approvals: u64,
    #[serde(default)]
    pub enable_approvals_whitelist: bool,
    #[serde(default)]
    pub approvals_whitelist_username: Vec<String>,
    #[serde(default)]
    pub approvals_whitelist_teams: Vec<String>,
    #[serde(default)]
    pub block_on_rejected_reviews: bool,
    #[serde(default)]
    pub block_on_official_review_requests: bool,
    #[serde(default)]
    pub block_on_outdated_branch: bool,
    #[serde(default)]
    pub dismiss_stale_approvals: bool,
    #[serde(default)]
    pub ignore_stale_approvals: bool,
    #[serde(default)]
    pub require_signed_commits: bool,
    #[serde(default)]
    pub protected_file_patterns: String,
    #[serde(default)]
    pub unprotected_file_patterns: String,
    #[serde(default = "default_true")]
    pub block_admin_merge_override: bool,
    #[serde(default)]
    pub enable_bypass_allowlist: bool,
}

impl Default for BranchProtectionRule {
    fn default() -> Self {
        Self {
            rule_name: "main".into(),
            enable_push: false,
            enable_push_whitelist: false,
            push_whitelist_usernames: Vec::new(),
            push_whitelist_teams: Vec::new(),
            push_whitelist_deploy_keys: false,
            enable_force_push: false,
            enable_force_push_allowlist: false,
            force_push_allowlist_usernames: Vec::new(),
            force_push_allowlist_teams: Vec::new(),
            force_push_allowlist_deploy_keys: false,
            enable_merge_whitelist: false,
            merge_whitelist_usernames: Vec::new(),
            merge_whitelist_teams: Vec::new(),
            enable_status_check: false,
            status_check_contexts: Vec::new(),
            required_approvals: 0,
            enable_approvals_whitelist: false,
            approvals_whitelist_username: Vec::new(),
            approvals_whitelist_teams: Vec::new(),
            block_on_rejected_reviews: false,
            block_on_official_review_requests: false,
            block_on_outdated_branch: false,
            dismiss_stale_approvals: false,
            ignore_stale_approvals: false,
            require_signed_commits: false,
            protected_file_patterns: String::new(),
            unprotected_file_patterns: String::new(),
            block_admin_merge_override: true,
            enable_bypass_allowlist: false,
        }
    }
}

/// Tag protection rule (`shako.protection_rule` with `kind = 'tag'`, S5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TagProtectionRule {
    pub name_pattern: String,
    #[serde(default)]
    pub whitelist_usernames: Vec<String>,
}

/// Stored protection rule record (`shako.protection_rule`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionRuleRecord {
    pub id: uuid::Uuid,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub repo_id: RepoId,
    pub rule: BranchProtectionRule,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Stored tag protection rule record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TagProtectionRecord {
    pub id: uuid::Uuid,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub repo_id: RepoId,
    pub rule: TagProtectionRule,
    pub created_at: DateTime<Utc>,
}

/// Pull request record (`shako.pull_request`, S10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestRecord {
    pub id: uuid::Uuid,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub repo_id: RepoId,
    pub number: u64,
    pub state: String,
    pub title: String,
    pub body: String,
    pub head_ref: String,
    pub head_sha: String,
    pub base_ref: String,
    pub base_sha: String,
    pub merge_base: String,
    pub mergeable: Option<bool>,
    pub merged: bool,
    pub merge_commit_sha: String,
    pub author_login: String,
    pub author_name: String,
    pub author_email: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// Review verdict (`APPROVED`, `REQUEST_CHANGES`, `COMMENT`, S10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ReviewVerdict {
    #[serde(rename = "APPROVED")]
    Approved,
    #[serde(rename = "REQUEST_CHANGES")]
    RequestChanges,
    #[serde(rename = "COMMENT")]
    Comment,
}

impl ReviewVerdict {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "APPROVED",
            Self::RequestChanges => "REQUEST_CHANGES",
            Self::Comment => "COMMENT",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "APPROVED" => Some(Self::Approved),
            "REQUEST_CHANGES" => Some(Self::RequestChanges),
            "COMMENT" => Some(Self::Comment),
            _ => None,
        }
    }
}

/// Review record (`shako.review`, S10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRecord {
    pub id: uuid::Uuid,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub repo_id: RepoId,
    pub pr_number: u64,
    pub verdict: ReviewVerdict,
    pub body: String,
    pub author_login: String,
    pub commit_id: String,
    pub dismissed: bool,
    pub created_at: DateTime<Utc>,
}

/// Commit status record (`shako.commit_status`, S11 row 23).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommitStatusRecord {
    pub id: uuid::Uuid,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub repo_id: RepoId,
    pub sha: String,
    pub context: String,
    pub state: String,
    pub description: String,
    pub target_url: String,
    pub updated_at: DateTime<Utc>,
}

/// Webhook record (`shako.webhook`, S14).
/// Never stores raw secrets: only `secret_ref` and `previous_secret_ref`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookRecord {
    pub id: WebhookId,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub repo_id: RepoId,
    pub url: String,
    pub events: Vec<String>,
    pub active: bool,
    #[serde(skip_serializing, default)]
    pub secret_ref: String,
    #[serde(skip_serializing)]
    pub previous_secret_ref: Option<String>,
    /// How a delivery is shaped and signed; a hook stored before formats existed is Gitea's.
    #[serde(default)]
    pub format: WebhookFormat,
    pub created_at: DateTime<Utc>,
}

/// The body and headers a webhook delivers: Gitea's (`X-Gitea-*`, Gitea's payload) for a hook an
/// outside client registered through the Gitea route, or shako's own (`X-Shako-*`, the native
/// payload) for one registered through OData or a manifest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WebhookFormat {
    #[default]
    Gitea,
    Shako,
}

/// Webhook delivery record (`shako.delivery`, S14). Never stores the response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryRecord {
    pub id: uuid::Uuid,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub webhook_id: WebhookId,
    pub event: String,
    pub status: String,
    pub response_code: Option<u16>,
    pub delivered_at: DateTime<Utc>,
}

/// Push mirror record (`shako.push_mirror`, S15).
/// Never exposes credentials in API/OData responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PushMirrorRecord {
    pub id: uuid::Uuid,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub repo_id: RepoId,
    pub remote_name: String,
    pub remote_address: String,
    #[serde(skip_serializing, default)]
    pub credential_secret_ref: String,
    pub interval: String,
    pub sync_on_commit: bool,
    pub last_sync: Option<DateTime<Utc>>,
    pub last_error: String,
    pub created_at: DateTime<Utc>,
}

/// Package record (`shako.package`, S13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PackageRecord {
    pub id: PackageId,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub owner: String,
    pub package_type: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

/// Package version record (`shako.package_version`, S13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PackageVersionRecord {
    pub id: uuid::Uuid,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub package_id: PackageId,
    pub owner: String,
    pub package_type: String,
    pub package_name: String,
    pub version: String,
    pub digest: String,
    pub media_type: String,
    /// Kept out of every API answer; the store log still writes it (shako-store-pg `hidden`).
    #[serde(skip_serializing, default)]
    pub manifest_bytes: Vec<u8>,
    pub subject_digest: Option<String>,
    pub artifact_type: Option<String>,
    pub size_bytes: u64,
    pub signed: bool,
    pub created_at: DateTime<Utc>,
}

/// Generic package file record (`shako.package_file`, S13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PackageFileRecord {
    pub id: uuid::Uuid,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub version_id: uuid::Uuid,
    pub owner: String,
    pub package_name: String,
    pub version: String,
    pub filename: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
}

/// Registry blob metadata record (`shako.blob`, S13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlobRecord {
    pub id: uuid::Uuid,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub owner: String,
    pub repo_name: String,
    pub digest: String,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
}

/// Quota usage record (`shako.quota_usage`, S6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuotaUsageRecord {
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub repo_count: u64,
    pub git_bytes: u64,
    pub package_bytes: u64,
    pub updated_at: DateTime<Utc>,
}

impl QuotaUsageRecord {
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.git_bytes.saturating_add(self.package_bytes)
    }
}

/// Transactional outbox event (`shako.outbox`, S14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutboxEvent {
    pub id: uuid::Uuid,
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub repo_id: Option<RepoId>,
    pub event_kind: String,
    pub idempotency_key: String,
    pub payload: Value,
    pub dispatched: bool,
    pub created_at: DateTime<Utc>,
    pub dispatched_at: Option<DateTime<Utc>>,
}
