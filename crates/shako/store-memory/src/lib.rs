#![forbid(unsafe_code)]
//! `shako-store-memory`: In-memory implementation of [`shako_core::Store`] with strict
//! per-`(InstanceId, OrgId)` isolation and simulated boot RLS checks (Contract S2).

use chrono::Utc;
use parking_lot::RwLock;
use shako_core::{
    BlobRecord, CommitStatusRecord, DeliveryRecord, InstanceId, InstanceRecord, OrgId, OutboxEvent,
    PackageFileRecord, PackageRecord, PackageVersionRecord, ProtectionRuleRecord,
    PullRequestRecord, PushMirrorRecord, QuotaUsageRecord, RepoId, RepoRecord, ReviewRecord,
    ShakoError, Store, StoreFuture, TagProtectionRecord, WebhookId, WebhookRecord,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
struct TenantState {
    instance: Option<InstanceRecord>,
    quota: QuotaUsageRecord,
    repos: BTreeMap<RepoId, RepoRecord>,
    protection_rules: BTreeMap<(RepoId, String), ProtectionRuleRecord>,
    tag_protections: BTreeMap<(RepoId, String), TagProtectionRecord>,
    pull_requests: BTreeMap<(RepoId, u64), PullRequestRecord>,
    reviews: Vec<ReviewRecord>,
    commit_statuses: BTreeMap<(RepoId, String, String), CommitStatusRecord>,
    webhooks: BTreeMap<WebhookId, WebhookRecord>,
    deliveries: Vec<DeliveryRecord>,
    push_mirrors: BTreeMap<(RepoId, String), PushMirrorRecord>,
    packages: BTreeMap<(String, String, String), PackageRecord>,
    package_versions: BTreeMap<(String, String, String, String), PackageVersionRecord>,
    package_files: BTreeMap<(String, String, String, String), PackageFileRecord>,
    blobs: BTreeMap<(String, String, String), BlobRecord>,
    outbox: Vec<OutboxEvent>,
}

#[derive(Debug, Default)]
struct InnerStore {
    tenants: BTreeMap<(InstanceId, OrgId), TenantState>,
    unguarded_tables: BTreeSet<String>,
    role_has_bypassrls: bool,
}

/// Thread-safe in-memory [`Store`] implementation.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: RwLock<InnerStore>,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn simulate_table_without_forced_rls(&self, table_name: impl Into<String>) {
        self.inner
            .write()
            .unguarded_tables
            .insert(table_name.into());
    }

    pub fn simulate_bypassrls_role(&self, bypass: bool) {
        self.inner.write().role_has_bypassrls = bypass;
    }
}

impl Store for MemoryStore {
    fn verify_rls_enforced(&self) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            let guard = self.inner.read();
            if guard.role_has_bypassrls {
                return Err(ShakoError::RlsNotEnforced(
                    "database role shako_app has BYPASSRLS enabled".into(),
                ));
            }
            if let Some(tbl) = guard.unguarded_tables.iter().next() {
                return Err(ShakoError::RlsNotEnforced(format!(
                    "table '{tbl}' in schema shako lacks FORCE ROW LEVEL SECURITY"
                )));
            }
            Ok(())
        })
    }

    fn upsert_instance(&self, instance: InstanceRecord) -> StoreFuture<'_, InstanceRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((instance.instance_id, instance.org_id))
                .or_default();
            t.instance = Some(instance.clone());
            Ok(instance)
        })
    }

    fn get_instance(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Option<InstanceRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .and_then(|t| t.instance.clone()))
        })
    }

    fn get_quota_usage(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, QuotaUsageRecord> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| t.quota.clone())
                .unwrap_or_else(|| QuotaUsageRecord {
                    instance_id,
                    org_id,
                    repo_count: 0,
                    git_bytes: 0,
                    package_bytes: 0,
                    updated_at: Utc::now(),
                }))
        })
    }

    fn adjust_quota_usage(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_delta: i64,
        git_bytes_delta: i64,
        package_bytes_delta: i64,
    ) -> StoreFuture<'_, QuotaUsageRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard.tenants.entry((instance_id, org_id)).or_default();
            t.quota.instance_id = instance_id;
            t.quota.org_id = org_id;
            t.quota.repo_count = (t.quota.repo_count as i64 + repo_delta).max(0) as u64;
            t.quota.git_bytes = (t.quota.git_bytes as i64 + git_bytes_delta).max(0) as u64;
            t.quota.package_bytes =
                (t.quota.package_bytes as i64 + package_bytes_delta).max(0) as u64;
            t.quota.updated_at = Utc::now();
            Ok(t.quota.clone())
        })
    }

    fn create_repo(&self, repo: RepoRecord) -> StoreFuture<'_, RepoRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((repo.instance_id, repo.org_id))
                .or_default();
            if t.repos
                .values()
                .any(|r| r.owner == repo.owner && r.name == repo.name)
            {
                return Err(ShakoError::Conflict("repository already exists".into()));
            }
            t.repos.insert(repo.id, repo.clone());
            Ok(repo)
        })
    }

    fn update_repo(&self, mut repo: RepoRecord) -> StoreFuture<'_, RepoRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((repo.instance_id, repo.org_id))
                .or_default();
            if !t.repos.contains_key(&repo.id) {
                return Err(ShakoError::NotFound("repository not found".into()));
            }
            repo.updated_at = Utc::now();
            t.repos.insert(repo.id, repo.clone());
            Ok(repo)
        })
    }

    fn get_repo_by_name(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        name: &str,
    ) -> StoreFuture<'_, Option<RepoRecord>> {
        let owner = owner.to_owned();
        let name = name.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard.tenants.get(&(instance_id, org_id)).and_then(|t| {
                t.repos
                    .values()
                    .find(|r| r.owner == owner && r.name == name)
                    .cloned()
            }))
        })
    }

    fn get_repo_by_id(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Option<RepoRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .and_then(|t| t.repos.get(&repo_id).cloned()))
        })
    }

    fn list_repos(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<RepoRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| t.repos.values().cloned().collect())
                .unwrap_or_default())
        })
    }

    fn delete_repo(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, bool> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let Some(t) = guard.tenants.get_mut(&(instance_id, org_id)) else {
                return Ok(false);
            };
            let removed = t.repos.remove(&repo_id).is_some();
            if removed {
                t.protection_rules.retain(|(rid, _), _| *rid != repo_id);
                t.tag_protections.retain(|(rid, _), _| *rid != repo_id);
                t.pull_requests.retain(|(rid, _), _| *rid != repo_id);
                t.reviews.retain(|r| r.repo_id != repo_id);
                t.push_mirrors.retain(|(rid, _), _| *rid != repo_id);
            }
            Ok(removed)
        })
    }

    fn allocate_pr_number(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, u64> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let repo = guard
                .tenants
                .get_mut(&(instance_id, org_id))
                .and_then(|t| t.repos.get_mut(&repo_id))
                .ok_or_else(|| ShakoError::NotFound("repository not found".into()))?;
            let num = repo.next_pr_number.max(1);
            repo.next_pr_number = num + 1;
            Ok(num)
        })
    }

    fn upsert_protection_rule(
        &self,
        mut rec: ProtectionRuleRecord,
    ) -> StoreFuture<'_, ProtectionRuleRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((rec.instance_id, rec.org_id))
                .or_default();
            rec.updated_at = Utc::now();
            t.protection_rules
                .insert((rec.repo_id, rec.rule.rule_name.clone()), rec.clone());
            Ok(rec)
        })
    }

    fn get_protection_rule(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        rule_name: &str,
    ) -> StoreFuture<'_, Option<ProtectionRuleRecord>> {
        let rname = rule_name.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .and_then(|t| t.protection_rules.get(&(repo_id, rname)).cloned()))
        })
    }

    fn list_protection_rules(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Vec<ProtectionRuleRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| {
                    t.protection_rules
                        .iter()
                        .filter(|((rid, _), _)| *rid == repo_id)
                        .map(|(_, v)| v.clone())
                        .collect()
                })
                .unwrap_or_default())
        })
    }

    fn delete_protection_rule(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        rule_name: &str,
    ) -> StoreFuture<'_, bool> {
        let rname = rule_name.to_owned();
        Box::pin(async move {
            let mut guard = self.inner.write();
            Ok(guard
                .tenants
                .get_mut(&(instance_id, org_id))
                .is_some_and(|t| t.protection_rules.remove(&(repo_id, rname)).is_some()))
        })
    }

    fn upsert_tag_protection(
        &self,
        rec: TagProtectionRecord,
    ) -> StoreFuture<'_, TagProtectionRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((rec.instance_id, rec.org_id))
                .or_default();
            t.tag_protections
                .insert((rec.repo_id, rec.rule.name_pattern.clone()), rec.clone());
            Ok(rec)
        })
    }

    fn list_tag_protections(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Vec<TagProtectionRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| {
                    t.tag_protections
                        .iter()
                        .filter(|((rid, _), _)| *rid == repo_id)
                        .map(|(_, v)| v.clone())
                        .collect()
                })
                .unwrap_or_default())
        })
    }

    fn create_pull_request(&self, pr: PullRequestRecord) -> StoreFuture<'_, PullRequestRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((pr.instance_id, pr.org_id))
                .or_default();
            t.pull_requests.insert((pr.repo_id, pr.number), pr.clone());
            Ok(pr)
        })
    }

    fn update_pull_request(&self, mut pr: PullRequestRecord) -> StoreFuture<'_, PullRequestRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((pr.instance_id, pr.org_id))
                .or_default();
            pr.updated_at = Utc::now();
            t.pull_requests.insert((pr.repo_id, pr.number), pr.clone());
            Ok(pr)
        })
    }

    fn get_pull_request(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        number: u64,
    ) -> StoreFuture<'_, Option<PullRequestRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .and_then(|t| t.pull_requests.get(&(repo_id, number)).cloned()))
        })
    }

    fn list_pull_requests(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
        state: Option<&str>,
    ) -> StoreFuture<'_, Vec<PullRequestRecord>> {
        let st = state.map(str::to_owned);
        Box::pin(async move {
            let guard = self.inner.read();
            let Some(t) = guard.tenants.get(&(instance_id, org_id)) else {
                return Ok(Vec::new());
            };
            let mut list: Vec<PullRequestRecord> = t
                .pull_requests
                .values()
                .filter(|pr| {
                    repo_id.is_none_or(|rid| pr.repo_id == rid)
                        && st
                            .as_deref()
                            .is_none_or(|s| s == "all" || pr.state.eq_ignore_ascii_case(s))
                })
                .cloned()
                .collect();
            list.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| b.number.cmp(&a.number))
            });
            Ok(list)
        })
    }

    fn create_review(&self, review: ReviewRecord) -> StoreFuture<'_, ReviewRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((review.instance_id, review.org_id))
                .or_default();
            t.reviews.push(review.clone());
            Ok(review)
        })
    }

    fn list_reviews(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
        pr_number: Option<u64>,
    ) -> StoreFuture<'_, Vec<ReviewRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| {
                    t.reviews
                        .iter()
                        .filter(|r| {
                            repo_id.is_none_or(|rid| r.repo_id == rid)
                                && pr_number.is_none_or(|n| r.pr_number == n)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default())
        })
    }

    fn upsert_commit_status(
        &self,
        status: CommitStatusRecord,
    ) -> StoreFuture<'_, CommitStatusRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((status.instance_id, status.org_id))
                .or_default();
            t.commit_statuses.insert(
                (status.repo_id, status.sha.clone(), status.context.clone()),
                status.clone(),
            );
            Ok(status)
        })
    }

    fn list_commit_statuses(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        sha: &str,
    ) -> StoreFuture<'_, Vec<CommitStatusRecord>> {
        let sha = sha.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| {
                    t.commit_statuses
                        .iter()
                        .filter(|((rid, s, _), _)| *rid == repo_id && s == &sha)
                        .map(|(_, v)| v.clone())
                        .collect()
                })
                .unwrap_or_default())
        })
    }

    fn create_webhook(&self, hook: WebhookRecord) -> StoreFuture<'_, WebhookRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((hook.instance_id, hook.org_id))
                .or_default();
            t.webhooks.insert(hook.id, hook.clone());
            Ok(hook)
        })
    }

    fn get_webhook(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        webhook_id: WebhookId,
    ) -> StoreFuture<'_, Option<WebhookRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .and_then(|t| t.webhooks.get(&webhook_id).cloned()))
        })
    }

    fn list_webhooks(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
    ) -> StoreFuture<'_, Vec<WebhookRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| {
                    t.webhooks
                        .values()
                        .filter(|h| repo_id.is_none_or(|rid| h.repo_id == rid))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default())
        })
    }

    fn record_delivery(&self, delivery: DeliveryRecord) -> StoreFuture<'_, DeliveryRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((delivery.instance_id, delivery.org_id))
                .or_default();
            t.deliveries.push(delivery.clone());
            Ok(delivery)
        })
    }

    fn list_deliveries(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        webhook_id: Option<WebhookId>,
    ) -> StoreFuture<'_, Vec<DeliveryRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| {
                    t.deliveries
                        .iter()
                        .filter(|d| webhook_id.is_none_or(|wid| d.webhook_id == wid))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default())
        })
    }

    fn upsert_push_mirror(&self, mirror: PushMirrorRecord) -> StoreFuture<'_, PushMirrorRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((mirror.instance_id, mirror.org_id))
                .or_default();
            t.push_mirrors
                .insert((mirror.repo_id, mirror.remote_name.clone()), mirror.clone());
            Ok(mirror)
        })
    }

    fn list_push_mirrors(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
    ) -> StoreFuture<'_, Vec<PushMirrorRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| {
                    t.push_mirrors
                        .iter()
                        .filter(|((rid, _), _)| repo_id.is_none_or(|r| *rid == r))
                        .map(|(_, v)| v.clone())
                        .collect()
                })
                .unwrap_or_default())
        })
    }

    fn delete_push_mirror(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        remote_name: &str,
    ) -> StoreFuture<'_, bool> {
        let rname = remote_name.to_owned();
        Box::pin(async move {
            let mut guard = self.inner.write();
            Ok(guard
                .tenants
                .get_mut(&(instance_id, org_id))
                .is_some_and(|t| t.push_mirrors.remove(&(repo_id, rname)).is_some()))
        })
    }

    fn upsert_package(&self, pkg: PackageRecord) -> StoreFuture<'_, PackageRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((pkg.instance_id, pkg.org_id))
                .or_default();
            let key = (
                pkg.owner.clone(),
                pkg.package_type.clone(),
                pkg.name.clone(),
            );
            if let Some(existing) = t.packages.get(&key) {
                return Ok(existing.clone());
            }
            t.packages.insert(key, pkg.clone());
            Ok(pkg)
        })
    }

    fn list_packages(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<PackageRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| t.packages.values().cloned().collect())
                .unwrap_or_default())
        })
    }

    fn upsert_package_version(
        &self,
        ver: PackageVersionRecord,
    ) -> StoreFuture<'_, PackageVersionRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((ver.instance_id, ver.org_id))
                .or_default();
            t.package_versions.insert(
                (
                    ver.owner.clone(),
                    ver.package_type.clone(),
                    ver.package_name.clone(),
                    ver.version.clone(),
                ),
                ver.clone(),
            );
            Ok(ver)
        })
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
        let owner = owner.to_owned();
        let pt = package_type.to_owned();
        let pn = package_name.to_owned();
        let vd = version_or_digest.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            let Some(t) = guard.tenants.get(&(instance_id, org_id)) else {
                return Ok(None);
            };
            if let Some(found) =
                t.package_versions
                    .get(&(owner.clone(), pt.clone(), pn.clone(), vd.clone()))
            {
                return Ok(Some(found.clone()));
            }
            Ok(t.package_versions
                .values()
                .find(|v| {
                    v.owner == owner
                        && v.package_type == pt
                        && v.package_name == pn
                        && v.digest == vd
                })
                .cloned())
        })
    }

    fn list_package_versions(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: Option<&str>,
        package_type: Option<&str>,
        package_name: Option<&str>,
    ) -> StoreFuture<'_, Vec<PackageVersionRecord>> {
        let ow = owner.map(str::to_owned);
        let pt = package_type.map(str::to_owned);
        let pn = package_name.map(str::to_owned);
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| {
                    t.package_versions
                        .values()
                        .filter(|v| {
                            ow.as_deref().is_none_or(|o| v.owner == o)
                                && pt.as_deref().is_none_or(|p| v.package_type == p)
                                && pn.as_deref().is_none_or(|n| v.package_name == n)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default())
        })
    }

    fn insert_package_file(&self, file: PackageFileRecord) -> StoreFuture<'_, PackageFileRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((file.instance_id, file.org_id))
                .or_default();
            let key = (
                file.owner.clone(),
                file.package_name.clone(),
                file.version.clone(),
                file.filename.clone(),
            );
            if t.package_files.contains_key(&key) {
                return Err(ShakoError::Conflict("package file already exists".into()));
            }
            t.package_files.insert(key, file.clone());
            Ok(file)
        })
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
        let key = (
            owner.to_owned(),
            package_name.to_owned(),
            version.to_owned(),
            filename.to_owned(),
        );
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .and_then(|t| t.package_files.get(&key).cloned()))
        })
    }

    fn list_package_files(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<PackageFileRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| t.package_files.values().cloned().collect())
                .unwrap_or_default())
        })
    }

    fn upsert_blob(&self, blob: BlobRecord) -> StoreFuture<'_, BlobRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((blob.instance_id, blob.org_id))
                .or_default();
            t.blobs.insert(
                (
                    blob.owner.clone(),
                    blob.repo_name.clone(),
                    blob.digest.clone(),
                ),
                blob.clone(),
            );
            Ok(blob)
        })
    }

    fn get_blob(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        repo_name: Option<&str>,
        digest: &str,
    ) -> StoreFuture<'_, Option<BlobRecord>> {
        let ow = owner.to_owned();
        let rn = repo_name.map(str::to_owned);
        let dg = digest.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            let Some(t) = guard.tenants.get(&(instance_id, org_id)) else {
                return Ok(None);
            };
            if let Some(repo) = rn {
                return Ok(t.blobs.get(&(ow, repo, dg)).cloned());
            }
            // Cross-repo mount lookup within the SAME owner only (Contract S13, Threat T13)
            Ok(t.blobs
                .iter()
                .find(|((o, _, d), _)| o == &ow && d == &dg)
                .map(|(_, b)| b.clone()))
        })
    }

    fn append_outbox(&self, event: OutboxEvent) -> StoreFuture<'_, OutboxEvent> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let t = guard
                .tenants
                .entry((event.instance_id, event.org_id))
                .or_default();
            if let Some(existing) = t
                .outbox
                .iter()
                .find(|e| e.idempotency_key == event.idempotency_key)
            {
                return Ok(existing.clone());
            }
            t.outbox.push(event.clone());
            Ok(event)
        })
    }

    fn list_undispatched_outbox(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<OutboxEvent>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| t.outbox.iter().filter(|e| !e.dispatched).cloned().collect())
                .unwrap_or_default())
        })
    }

    fn mark_outbox_dispatched(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        event_id: uuid::Uuid,
    ) -> StoreFuture<'_, bool> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let Some(t) = guard.tenants.get_mut(&(instance_id, org_id)) else {
                return Ok(false);
            };
            if let Some(ev) = t.outbox.iter_mut().find(|e| e.id == event_id) {
                ev.dispatched = true;
                ev.dispatched_at = Some(Utc::now());
                return Ok(true);
            }
            Ok(false)
        })
    }

    fn list_outbox_events(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<OutboxEvent>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .tenants
                .get(&(instance_id, org_id))
                .map(|t| t.outbox.clone())
                .unwrap_or_default())
        })
    }
}
