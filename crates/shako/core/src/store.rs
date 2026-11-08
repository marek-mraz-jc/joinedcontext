#![forbid(unsafe_code)]
//! Async `Store` and `SecretStore` traits implemented by `shako-store-pg` and `shako-store-memory` (S2).

use crate::error::ShakoResult;
use crate::ids::{InstanceId, OrgId, RepoId, WebhookId};
use crate::model::*;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use parking_lot::RwLock;

pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = ShakoResult<T>> + Send + 'a>>;

/// Resolves secrets (`secretRef`) from the secrets mount without storing raw secrets in database tables (S14, S15, S21).
pub trait SecretStore: Send + Sync {
    fn get_secret(&self, org_id: OrgId, secret_ref: &str) -> Option<String>;
    fn put_secret(&self, org_id: OrgId, secret_ref: &str, value: &str);
}

/// In-memory / file-backed secret store.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    secrets: RwLock<BTreeMap<(OrgId, String), String>>,
}

impl MemorySecretStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn get_secret(&self, org_id: OrgId, secret_ref: &str) -> Option<String> {
        self.secrets
            .read()
            .get(&(org_id, secret_ref.to_owned()))
            .cloned()
    }

    fn put_secret(&self, org_id: OrgId, secret_ref: &str, value: &str) {
        self.secrets
            .write()
            .insert((org_id, secret_ref.to_owned()), value.to_owned());
    }
}

/// Storage interface for all `shako` metadata, scoped per `(InstanceId, OrgId)` (S2).
pub trait Store: Send + Sync {
    /// Verifies that forced RLS is active on every table in `shako` and the request role
    /// does not have `BYPASSRLS` or `SUPERUSER` (S2).
    fn verify_rls_enforced(&self) -> StoreFuture<'_, ()>;

    // Instances & Quotas
    fn upsert_instance(&self, instance: InstanceRecord) -> StoreFuture<'_, InstanceRecord>;
    fn get_instance(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Option<InstanceRecord>>;
    fn get_quota_usage(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, QuotaUsageRecord>;
    fn adjust_quota_usage(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_delta: i64,
        git_bytes_delta: i64,
        package_bytes_delta: i64,
    ) -> StoreFuture<'_, QuotaUsageRecord>;

    // Repositories
    fn create_repo(&self, repo: RepoRecord) -> StoreFuture<'_, RepoRecord>;
    fn update_repo(&self, repo: RepoRecord) -> StoreFuture<'_, RepoRecord>;
    fn get_repo_by_name(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        name: &str,
    ) -> StoreFuture<'_, Option<RepoRecord>>;
    fn get_repo_by_id(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Option<RepoRecord>>;
    fn list_repos(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<RepoRecord>>;
    fn delete_repo(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, bool>;
    fn allocate_pr_number(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, u64>;

    // Branch & Tag Protection Rules
    fn upsert_protection_rule(
        &self,
        rec: ProtectionRuleRecord,
    ) -> StoreFuture<'_, ProtectionRuleRecord>;
    fn get_protection_rule(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        rule_name: &str,
    ) -> StoreFuture<'_, Option<ProtectionRuleRecord>>;
    fn list_protection_rules(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Vec<ProtectionRuleRecord>>;
    fn delete_protection_rule(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        rule_name: &str,
    ) -> StoreFuture<'_, bool>;

    fn upsert_tag_protection(&self, rec: TagProtectionRecord) -> StoreFuture<'_, TagProtectionRecord>;
    fn list_tag_protections(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Vec<TagProtectionRecord>>;

    // Pull Requests & Reviews
    fn create_pull_request(&self, pr: PullRequestRecord) -> StoreFuture<'_, PullRequestRecord>;
    fn update_pull_request(&self, pr: PullRequestRecord) -> StoreFuture<'_, PullRequestRecord>;
    fn get_pull_request(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        number: u64,
    ) -> StoreFuture<'_, Option<PullRequestRecord>>;
    fn list_pull_requests(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
        state: Option<&str>,
    ) -> StoreFuture<'_, Vec<PullRequestRecord>>;

    fn create_review(&self, review: ReviewRecord) -> StoreFuture<'_, ReviewRecord>;
    fn list_reviews(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
        pr_number: Option<u64>,
    ) -> StoreFuture<'_, Vec<ReviewRecord>>;

    // Commit Statuses
    fn upsert_commit_status(
        &self,
        status: CommitStatusRecord,
    ) -> StoreFuture<'_, CommitStatusRecord>;
    fn list_commit_statuses(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        sha: &str,
    ) -> StoreFuture<'_, Vec<CommitStatusRecord>>;

    // Webhooks & Deliveries
    fn create_webhook(&self, hook: WebhookRecord) -> StoreFuture<'_, WebhookRecord>;
    fn get_webhook(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        webhook_id: WebhookId,
    ) -> StoreFuture<'_, Option<WebhookRecord>>;
    fn list_webhooks(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
    ) -> StoreFuture<'_, Vec<WebhookRecord>>;
    fn record_delivery(&self, delivery: DeliveryRecord) -> StoreFuture<'_, DeliveryRecord>;
    fn list_deliveries(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        webhook_id: Option<WebhookId>,
    ) -> StoreFuture<'_, Vec<DeliveryRecord>>;

    // Push Mirrors
    fn upsert_push_mirror(&self, mirror: PushMirrorRecord) -> StoreFuture<'_, PushMirrorRecord>;
    fn list_push_mirrors(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
    ) -> StoreFuture<'_, Vec<PushMirrorRecord>>;
    fn delete_push_mirror(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        remote_name: &str,
    ) -> StoreFuture<'_, bool>;

    // Packages & OCI Blobs
    fn upsert_package(&self, pkg: PackageRecord) -> StoreFuture<'_, PackageRecord>;
    fn list_packages(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<PackageRecord>>;
    fn upsert_package_version(
        &self,
        ver: PackageVersionRecord,
    ) -> StoreFuture<'_, PackageVersionRecord>;
    fn get_package_version(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        package_type: &str,
        package_name: &str,
        version_or_digest: &str,
    ) -> StoreFuture<'_, Option<PackageVersionRecord>>;
    fn list_package_versions(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: Option<&str>,
        package_type: Option<&str>,
        package_name: Option<&str>,
    ) -> StoreFuture<'_, Vec<PackageVersionRecord>>;
    fn insert_package_file(&self, file: PackageFileRecord) -> StoreFuture<'_, PackageFileRecord>;
    fn get_package_file(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        package_name: &str,
        version: &str,
        filename: &str,
    ) -> StoreFuture<'_, Option<PackageFileRecord>>;
    fn list_package_files(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<PackageFileRecord>>;

    fn upsert_blob(&self, blob: BlobRecord) -> StoreFuture<'_, BlobRecord>;
    fn get_blob(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        repo_name: Option<&str>,
        digest: &str,
    ) -> StoreFuture<'_, Option<BlobRecord>>;

    // Outbox
    fn append_outbox(&self, event: OutboxEvent) -> StoreFuture<'_, OutboxEvent>;
    fn list_undispatched_outbox(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<OutboxEvent>>;
    fn mark_outbox_dispatched(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        event_id: uuid::Uuid,
    ) -> StoreFuture<'_, bool>;
    fn list_outbox_events(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<OutboxEvent>>;
}
