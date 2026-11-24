//! A [`MemoryStore`] whose calls a test names answer an internal error. Shared by the test targets
//! of shako-api (`mod support`) and shako-registry (`#[path]`), so each fault test reaches the
//! store-error branch of a handler without a second copy.

use parking_lot::Mutex;
use shako_core::model::*;
use shako_core::store::{Store, StoreFuture};
use shako_core::{InstanceId, OrgId, RepoId, ShakoError, WebhookId};
use shako_store_memory::MemoryStore;
use std::collections::BTreeSet;

/// A [`MemoryStore`] whose calls named in `failing` answer an internal error.
#[derive(Default)]
pub struct FaultStore {
    inner: MemoryStore,
    failing: Mutex<BTreeSet<&'static str>>,
}

impl FaultStore {
    pub fn fail(&self, call: &'static str) {
        self.failing.lock().insert(call);
    }

    pub fn heal(&self) {
        self.failing.lock().clear();
    }

    fn fails(&self, call: &str) -> bool {
        self.failing.lock().contains(call)
    }
}

pub fn refused<T: Send + 'static>(call: &'static str) -> StoreFuture<'static, T> {
    Box::pin(async move {
        Err(ShakoError::Internal(format!(
            "{call} refused by the test store"
        )))
    })
}

impl Store for FaultStore {
    fn verify_rls_enforced(&self) -> StoreFuture<'_, ()> {
        if self.fails("verify_rls_enforced") {
            return refused("verify_rls_enforced");
        }
        self.inner.verify_rls_enforced()
    }
    fn upsert_instance(&self, instance: InstanceRecord) -> StoreFuture<'_, InstanceRecord> {
        if self.fails("upsert_instance") {
            return refused("upsert_instance");
        }
        self.inner.upsert_instance(instance)
    }
    fn get_instance(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Option<InstanceRecord>> {
        if self.fails("get_instance") {
            return refused("get_instance");
        }
        self.inner.get_instance(instance_id, org_id)
    }
    fn get_quota_usage(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, QuotaUsageRecord> {
        if self.fails("get_quota_usage") {
            return refused("get_quota_usage");
        }
        self.inner.get_quota_usage(instance_id, org_id)
    }
    fn adjust_quota_usage(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_delta: i64,
        git_bytes_delta: i64,
        package_bytes_delta: i64,
    ) -> StoreFuture<'_, QuotaUsageRecord> {
        if self.fails("adjust_quota_usage") {
            return refused("adjust_quota_usage");
        }
        self.inner.adjust_quota_usage(
            instance_id,
            org_id,
            repo_delta,
            git_bytes_delta,
            package_bytes_delta,
        )
    }
    fn create_repo(&self, repo: RepoRecord) -> StoreFuture<'_, RepoRecord> {
        if self.fails("create_repo") {
            return refused("create_repo");
        }
        self.inner.create_repo(repo)
    }
    fn update_repo(&self, repo: RepoRecord) -> StoreFuture<'_, RepoRecord> {
        if self.fails("update_repo") {
            return refused("update_repo");
        }
        self.inner.update_repo(repo)
    }
    fn get_repo_by_name(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        name: &str,
    ) -> StoreFuture<'_, Option<RepoRecord>> {
        if self.fails("get_repo_by_name") {
            return refused("get_repo_by_name");
        }
        self.inner
            .get_repo_by_name(instance_id, org_id, owner, name)
    }
    fn get_repo_by_id(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Option<RepoRecord>> {
        if self.fails("get_repo_by_id") {
            return refused("get_repo_by_id");
        }
        self.inner.get_repo_by_id(instance_id, org_id, repo_id)
    }
    fn list_repos(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<RepoRecord>> {
        if self.fails("list_repos") {
            return refused("list_repos");
        }
        self.inner.list_repos(instance_id, org_id)
    }
    fn delete_repo(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, bool> {
        if self.fails("delete_repo") {
            return refused("delete_repo");
        }
        self.inner.delete_repo(instance_id, org_id, repo_id)
    }
    fn allocate_pr_number(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, u64> {
        if self.fails("allocate_pr_number") {
            return refused("allocate_pr_number");
        }
        self.inner.allocate_pr_number(instance_id, org_id, repo_id)
    }
    fn upsert_protection_rule(
        &self,
        rec: ProtectionRuleRecord,
    ) -> StoreFuture<'_, ProtectionRuleRecord> {
        if self.fails("upsert_protection_rule") {
            return refused("upsert_protection_rule");
        }
        self.inner.upsert_protection_rule(rec)
    }
    fn get_protection_rule(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        rule_name: &str,
    ) -> StoreFuture<'_, Option<ProtectionRuleRecord>> {
        if self.fails("get_protection_rule") {
            return refused("get_protection_rule");
        }
        self.inner
            .get_protection_rule(instance_id, org_id, repo_id, rule_name)
    }
    fn list_protection_rules(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Vec<ProtectionRuleRecord>> {
        if self.fails("list_protection_rules") {
            return refused("list_protection_rules");
        }
        self.inner
            .list_protection_rules(instance_id, org_id, repo_id)
    }
    fn delete_protection_rule(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        rule_name: &str,
    ) -> StoreFuture<'_, bool> {
        if self.fails("delete_protection_rule") {
            return refused("delete_protection_rule");
        }
        self.inner
            .delete_protection_rule(instance_id, org_id, repo_id, rule_name)
    }
    fn upsert_tag_protection(
        &self,
        rec: TagProtectionRecord,
    ) -> StoreFuture<'_, TagProtectionRecord> {
        if self.fails("upsert_tag_protection") {
            return refused("upsert_tag_protection");
        }
        self.inner.upsert_tag_protection(rec)
    }
    fn list_tag_protections(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Vec<TagProtectionRecord>> {
        if self.fails("list_tag_protections") {
            return refused("list_tag_protections");
        }
        self.inner
            .list_tag_protections(instance_id, org_id, repo_id)
    }
    fn create_pull_request(&self, pr: PullRequestRecord) -> StoreFuture<'_, PullRequestRecord> {
        if self.fails("create_pull_request") {
            return refused("create_pull_request");
        }
        self.inner.create_pull_request(pr)
    }
    fn update_pull_request(&self, pr: PullRequestRecord) -> StoreFuture<'_, PullRequestRecord> {
        if self.fails("update_pull_request") {
            return refused("update_pull_request");
        }
        self.inner.update_pull_request(pr)
    }
    fn get_pull_request(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        number: u64,
    ) -> StoreFuture<'_, Option<PullRequestRecord>> {
        if self.fails("get_pull_request") {
            return refused("get_pull_request");
        }
        self.inner
            .get_pull_request(instance_id, org_id, repo_id, number)
    }
    fn list_pull_requests(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
        state: Option<&str>,
    ) -> StoreFuture<'_, Vec<PullRequestRecord>> {
        if self.fails("list_pull_requests") {
            return refused("list_pull_requests");
        }
        self.inner
            .list_pull_requests(instance_id, org_id, repo_id, state)
    }
    fn create_review(&self, review: ReviewRecord) -> StoreFuture<'_, ReviewRecord> {
        if self.fails("create_review") {
            return refused("create_review");
        }
        self.inner.create_review(review)
    }
    fn list_reviews(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
        pr_number: Option<u64>,
    ) -> StoreFuture<'_, Vec<ReviewRecord>> {
        if self.fails("list_reviews") {
            return refused("list_reviews");
        }
        self.inner
            .list_reviews(instance_id, org_id, repo_id, pr_number)
    }
    fn upsert_commit_status(
        &self,
        status: CommitStatusRecord,
    ) -> StoreFuture<'_, CommitStatusRecord> {
        if self.fails("upsert_commit_status") {
            return refused("upsert_commit_status");
        }
        self.inner.upsert_commit_status(status)
    }
    fn list_commit_statuses(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        sha: &str,
    ) -> StoreFuture<'_, Vec<CommitStatusRecord>> {
        if self.fails("list_commit_statuses") {
            return refused("list_commit_statuses");
        }
        self.inner
            .list_commit_statuses(instance_id, org_id, repo_id, sha)
    }
    fn create_webhook(&self, hook: WebhookRecord) -> StoreFuture<'_, WebhookRecord> {
        if self.fails("create_webhook") {
            return refused("create_webhook");
        }
        self.inner.create_webhook(hook)
    }
    fn get_webhook(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        webhook_id: WebhookId,
    ) -> StoreFuture<'_, Option<WebhookRecord>> {
        if self.fails("get_webhook") {
            return refused("get_webhook");
        }
        self.inner.get_webhook(instance_id, org_id, webhook_id)
    }
    fn list_webhooks(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
    ) -> StoreFuture<'_, Vec<WebhookRecord>> {
        if self.fails("list_webhooks") {
            return refused("list_webhooks");
        }
        self.inner.list_webhooks(instance_id, org_id, repo_id)
    }
    fn record_delivery(&self, delivery: DeliveryRecord) -> StoreFuture<'_, DeliveryRecord> {
        if self.fails("record_delivery") {
            return refused("record_delivery");
        }
        self.inner.record_delivery(delivery)
    }
    fn list_deliveries(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        webhook_id: Option<WebhookId>,
    ) -> StoreFuture<'_, Vec<DeliveryRecord>> {
        if self.fails("list_deliveries") {
            return refused("list_deliveries");
        }
        self.inner.list_deliveries(instance_id, org_id, webhook_id)
    }
    fn mark_mirror_due(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, i64> {
        if self.fails("mark_mirror_due") {
            return refused("mark_mirror_due");
        }
        self.inner.mark_mirror_due(instance_id, org_id, repo_id)
    }
    fn mirror_synced(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        mark: i64,
    ) -> StoreFuture<'_, bool> {
        if self.fails("mirror_synced") {
            return refused("mirror_synced");
        }
        self.inner.mirror_synced(instance_id, org_id, repo_id, mark)
    }
    fn claim_mirror_sync(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
    ) -> StoreFuture<'_, Option<(RepoId, i64)>> {
        if self.fails("claim_mirror_sync") {
            return refused("claim_mirror_sync");
        }
        self.inner.claim_mirror_sync(instance_id, org_id, repo_id)
    }
    fn finish_mirror_sync(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        mark: i64,
    ) -> StoreFuture<'_, ()> {
        if self.fails("finish_mirror_sync") {
            return refused("finish_mirror_sync");
        }
        self.inner
            .finish_mirror_sync(instance_id, org_id, repo_id, mark)
    }
    fn upsert_push_mirror(&self, mirror: PushMirrorRecord) -> StoreFuture<'_, PushMirrorRecord> {
        if self.fails("upsert_push_mirror") {
            return refused("upsert_push_mirror");
        }
        self.inner.upsert_push_mirror(mirror)
    }
    fn list_push_mirrors(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
    ) -> StoreFuture<'_, Vec<PushMirrorRecord>> {
        if self.fails("list_push_mirrors") {
            return refused("list_push_mirrors");
        }
        self.inner.list_push_mirrors(instance_id, org_id, repo_id)
    }
    fn delete_push_mirror(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        remote_name: &str,
    ) -> StoreFuture<'_, bool> {
        if self.fails("delete_push_mirror") {
            return refused("delete_push_mirror");
        }
        self.inner
            .delete_push_mirror(instance_id, org_id, repo_id, remote_name)
    }
    fn upsert_package(&self, pkg: PackageRecord) -> StoreFuture<'_, PackageRecord> {
        if self.fails("upsert_package") {
            return refused("upsert_package");
        }
        self.inner.upsert_package(pkg)
    }
    fn list_packages(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<PackageRecord>> {
        if self.fails("list_packages") {
            return refused("list_packages");
        }
        self.inner.list_packages(instance_id, org_id)
    }
    fn upsert_package_version(
        &self,
        ver: PackageVersionRecord,
    ) -> StoreFuture<'_, PackageVersionRecord> {
        if self.fails("upsert_package_version") {
            return refused("upsert_package_version");
        }
        self.inner.upsert_package_version(ver)
    }
    fn get_package_version(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        package_type: &str,
        package_name: &str,
        version_or_digest: &str,
    ) -> StoreFuture<'_, Option<PackageVersionRecord>> {
        if self.fails("get_package_version") {
            return refused("get_package_version");
        }
        self.inner.get_package_version(
            instance_id,
            org_id,
            owner,
            package_type,
            package_name,
            version_or_digest,
        )
    }
    fn list_package_versions(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: Option<&str>,
        package_type: Option<&str>,
        package_name: Option<&str>,
    ) -> StoreFuture<'_, Vec<PackageVersionRecord>> {
        if self.fails("list_package_versions") {
            return refused("list_package_versions");
        }
        self.inner
            .list_package_versions(instance_id, org_id, owner, package_type, package_name)
    }
    fn list_package_tags(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        image: &str,
        after: Option<&str>,
        limit: usize,
    ) -> StoreFuture<'_, Vec<String>> {
        if self.fails("list_package_tags") {
            return refused("list_package_tags");
        }
        self.inner
            .list_package_tags(instance_id, org_id, owner, image, after, limit)
    }
    fn list_referrers(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        image: &str,
        subject_digest: &str,
    ) -> StoreFuture<'_, Vec<PackageVersionRecord>> {
        if self.fails("list_referrers") {
            return refused("list_referrers");
        }
        self.inner
            .list_referrers(instance_id, org_id, owner, image, subject_digest)
    }
    fn insert_package_file(&self, file: PackageFileRecord) -> StoreFuture<'_, PackageFileRecord> {
        if self.fails("insert_package_file") {
            return refused("insert_package_file");
        }
        self.inner.insert_package_file(file)
    }
    fn get_package_file(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        package_name: &str,
        version: &str,
        filename: &str,
    ) -> StoreFuture<'_, Option<PackageFileRecord>> {
        if self.fails("get_package_file") {
            return refused("get_package_file");
        }
        self.inner
            .get_package_file(instance_id, org_id, owner, package_name, version, filename)
    }
    fn list_package_files(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<PackageFileRecord>> {
        if self.fails("list_package_files") {
            return refused("list_package_files");
        }
        self.inner.list_package_files(instance_id, org_id)
    }
    fn upsert_blob(&self, blob: BlobRecord) -> StoreFuture<'_, BlobRecord> {
        if self.fails("upsert_blob") {
            return refused("upsert_blob");
        }
        self.inner.upsert_blob(blob)
    }
    fn store_blob(
        &self,
        blob: BlobRecord,
        storage_limit: Option<u64>,
    ) -> StoreFuture<'_, BlobRecord> {
        if self.fails("store_blob") {
            return refused("store_blob");
        }
        self.inner.store_blob(blob, storage_limit)
    }
    fn get_blob(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        repo_name: Option<&str>,
        digest: &str,
    ) -> StoreFuture<'_, Option<BlobRecord>> {
        if self.fails("get_blob") {
            return refused("get_blob");
        }
        self.inner
            .get_blob(instance_id, org_id, owner, repo_name, digest)
    }
    fn append_outbox(&self, event: OutboxEvent) -> StoreFuture<'_, OutboxEvent> {
        if self.fails("append_outbox") {
            return refused("append_outbox");
        }
        self.inner.append_outbox(event)
    }
    fn list_undispatched_outbox(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<OutboxEvent>> {
        if self.fails("list_undispatched_outbox") {
            return refused("list_undispatched_outbox");
        }
        self.inner.list_undispatched_outbox(instance_id, org_id)
    }
    fn mark_outbox_dispatched(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        event_id: uuid::Uuid,
    ) -> StoreFuture<'_, bool> {
        if self.fails("mark_outbox_dispatched") {
            return refused("mark_outbox_dispatched");
        }
        if self.fails("mark_outbox_dispatched=false") {
            return Box::pin(async { Ok(false) });
        }
        self.inner
            .mark_outbox_dispatched(instance_id, org_id, event_id)
    }
    fn claim_outbox(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Option<OutboxClaim>> {
        if self.fails("claim_outbox") {
            return refused("claim_outbox");
        }
        self.inner.claim_outbox(instance_id, org_id)
    }
    fn retry_outbox(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        event_id: uuid::Uuid,
        after: std::time::Duration,
    ) -> StoreFuture<'_, bool> {
        if self.fails("retry_outbox") {
            return refused("retry_outbox");
        }
        self.inner
            .retry_outbox(instance_id, org_id, event_id, after)
    }
    fn list_outbox_events(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<OutboxEvent>> {
        if self.fails("list_outbox_events") {
            return refused("list_outbox_events");
        }
        self.inner.list_outbox_events(instance_id, org_id)
    }
    fn append_audit(&self, row: AuditRecord) -> StoreFuture<'_, ()> {
        if self.fails("append_audit") {
            return refused("append_audit");
        }
        self.inner.append_audit(row)
    }
    fn list_audit(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<AuditRecord>> {
        if self.fails("list_audit") {
            return refused("list_audit");
        }
        self.inner.list_audit(instance_id, org_id)
    }
}
