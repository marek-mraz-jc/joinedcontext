#![forbid(unsafe_code)]
//! The edges of the forge's surfaces: what it answers when its store or secret store fails after
//! a request resolved (a store that refuses the calls a test names: a 500 naming no store detail,
//! or the write standing when only its bookkeeping failed), and the variants of each request the
//! journeys do not take (defaults, lower-case argument names, an unknown ref, a refused style).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use parking_lot::Mutex;
use serde_json::{Value, json};
use shako_api::{ShakoState, compat_and_native_rest_router, odata_router, smart_http_router};
use shako_core::model::*;
use shako_core::store::{SecretStore, Store, StoreFuture};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, Principal, RepoId,
    RoleBinding, ShakoError, ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
    WebhookId,
};
use shako_store_memory::MemoryStore;
use std::collections::BTreeSet;
use std::sync::Arc;
use tower::ServiceExt;

const ADMIN: &str = "admin-token";
const OUTSIDER: &str = "outsider-token";
const R: &str = "/api/v1/repos/joinedcontext/site";

/// A [`MemoryStore`] whose calls named in `failing` answer an internal error.
#[derive(Default)]
struct FaultStore {
    inner: MemoryStore,
    failing: Mutex<BTreeSet<&'static str>>,
}

impl FaultStore {
    fn fail(&self, call: &'static str) {
        self.failing.lock().insert(call);
    }

    fn heal(&self) {
        self.failing.lock().clear();
    }

    fn fails(&self, call: &str) -> bool {
        self.failing.lock().contains(call)
    }
}

fn refused<T: Send + 'static>(call: &'static str) -> StoreFuture<'static, T> {
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

/// A secret store whose reads fail while `failing` is set; writes are kept.
#[derive(Default)]
struct FaultSecrets {
    inner: shako_core::store::MemorySecretStore,
    failing: Mutex<bool>,
}

impl SecretStore for FaultSecrets {
    fn get_secret<'a>(
        &'a self,
        org_id: OrgId,
        secret_ref: &'a str,
    ) -> StoreFuture<'a, Option<String>> {
        if *self.failing.lock() {
            return refused("get_secret");
        }
        self.inner.get_secret(org_id, secret_ref)
    }

    fn put_secret<'a>(
        &'a self,
        org_id: OrgId,
        secret_ref: &'a str,
        value: &'a str,
    ) -> StoreFuture<'a, ()> {
        self.inner.put_secret(org_id, secret_ref, value)
    }
}

struct World {
    app: Router,
    state: ShakoState,
    store: Arc<FaultStore>,
    secrets: Arc<FaultSecrets>,
    /// The headers of each hook delivery the outbound guard let out.
    deliveries: Arc<Mutex<Vec<Vec<(String, String)>>>>,
    _root: tempfile::TempDir,
}

fn world() -> World {
    let root = tempfile::tempdir().expect("repo root");
    let org = OrgId::from_u128(10);
    let authz = Arc::new(LocalAuthorizer::new(vec![
        RoleBinding {
            org_id: org,
            principal: Principal::User("admin".into()),
            actions: vec![
                Action::Admin,
                Action::Write,
                Action::Approve,
                Action::Publish,
            ],
            project_id: None,
            repo: None,
        },
        // A group that writes to `site` alone: a Team of that repository.
        RoleBinding {
            org_id: org,
            principal: Principal::Group("site-editors".into()),
            actions: vec![Action::Read, Action::Write],
            project_id: None,
            repo: Some("site".into()),
        },
    ]));
    let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
        .with_static_tokens(vec![
            StaticTokenBinding::new(ADMIN, Subject::authenticated("admin", "admin", vec![org])),
            // A member of the Organization with no binding: sees nothing.
            StaticTokenBinding::new(
                OUTSIDER,
                Subject::authenticated("outsider", "outsider", vec![org]),
            ),
        ]);
    let deliveries = Arc::new(Mutex::new(Vec::new()));
    let seen = deliveries.clone();
    let guard = OutboundGuard::new(OutboundPolicy {
        allowed_hosts: Some(vec!["github.com".into(), "hooks.city.example".into()]),
        mock_handler: Some(Arc::new(move |method, url, headers, body| {
            if url.contains("/broken") {
                return Some((500, Vec::new()));
            }
            if method == "GET" && url.ends_with("/info/refs?service=git-receive-pack") {
                let mut adv = pkt("# service=git-receive-pack\n");
                adv.extend_from_slice(b"0000");
                adv.extend(pkt(&format!(
                    "{} capabilities^{{}}\0report-status delete-refs ofs-delta\n",
                    "0".repeat(40)
                )));
                adv.extend_from_slice(b"0000");
                return Some((200, adv));
            }
            if method == "POST" && url.ends_with("/git-receive-pack") {
                return Some((200, report_status(body)));
            }
            if method == "POST" && url.starts_with("https://hooks.city.example/") {
                seen.lock().push(headers.to_vec());
                // A receiver that redirects is refused by the guard, for good.
                let status = if url.ends_with("/moved") { 302 } else { 200 };
                return Some((status, b"ok".to_vec()));
            }
            None
        })),
        ..OutboundPolicy::default()
    });
    let store = Arc::new(FaultStore::default());
    let secrets = Arc::new(FaultSecrets::default());
    let mut state = ShakoState::new(
        InstanceId::from_u128(1),
        root.path().to_path_buf(),
        store.clone(),
        authz,
        verifier,
        guard,
    );
    state.secret_store = secrets.clone();
    let app = compat_and_native_rest_router()
        .merge(odata_router())
        .merge(smart_http_router())
        .with_state(state.clone());
    World {
        app,
        state,
        store,
        secrets,
        deliveries,
        _root: root,
    }
}

async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    send_as(app, ADMIN, method, uri, body).await
}

/// The status and bytes of a request as it is.
async fn raw(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.expect("answer");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 24)
        .await
        .expect("body");
    (status, bytes.to_vec())
}

async fn send_as(
    app: &Router,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("token {token}"));
    let req = match body {
        Some(v) => req
            .header("content-type", "application/json")
            .body(Body::from(v.to_string())),
        None => req.body(Body::empty()),
    }
    .expect("request");
    let resp = app.clone().oneshot(req).await.expect("answer");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 24)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn put(content: &str, message: &str, branch: &str) -> Value {
    json!({ "content": STANDARD.encode(content), "message": message, "branch": branch,
        "author": { "name": "Ada", "email": "ada@city.example" } })
}

/// A repository `joinedcontext/site` with one file on `main`.
async fn seeded() -> World {
    let w = world();
    let (s, _) = send(
        &w.app,
        "POST",
        "/api/v1/orgs/joinedcontext/repos",
        Some(json!({ "name": "site" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/README.md"),
        Some(put("hello", "first", "main")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    w
}

/// `call` fails while `method uri` runs: the answer is a 500 naming no store detail.
async fn refused_when(
    w: &World,
    call: &'static str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Value {
    w.store.fail(call);
    let (status, answer) = send(&w.app, method, uri, body).await;
    w.store.heal();
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{call} during {method} {uri}: {answer}"
    );
    assert!(
        !answer.to_string().contains("refused by the test store"),
        "{answer}"
    );
    answer
}

const INSTANCE: InstanceId = InstanceId::from_u128(1);
const ORG: OrgId = OrgId::from_u128(10);

impl World {
    async fn repo(&self, name: &str) -> RepoRecord {
        self.state
            .store()
            .get_repo_by_name(INSTANCE, ORG, "joinedcontext", name)
            .await
            .expect("store")
            .expect("repository")
    }

    async fn disk(&self, name: &str) -> std::path::PathBuf {
        let repo = self.repo(name).await;
        shako_core::repo_disk_path(self.state.repo_root(), INSTANCE, repo.id).expect("disk path")
    }

    /// Runs git on the bare repository, as a push from outside would change it.
    async fn git(&self, name: &str, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(self.disk(name).await)
            .args(["-c", "user.name=Ada", "-c", "user.email=ada@city.example"])
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A branch `name` off main with one commit of its own.
    async fn branch_with_commit(&self, name: &str) {
        let (s, b) = send(
            &self.app,
            "POST",
            &format!("{R}/branches"),
            Some(json!({ "new_branch_name": name, "old_branch_name": "main" })),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        let (s, c) = send(
            &self.app,
            "PUT",
            &format!("{R}/contents/{name}.md"),
            Some(put(name, &format!("on {name}"), name)),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{c}");
    }

    async fn open_pull(&self, head: &str) -> u64 {
        self.branch_with_commit(head).await;
        let (s, pr) = send(
            &self.app,
            "POST",
            &format!("{R}/pulls"),
            Some(json!({ "head": head, "base": "main", "title": format!("from {head}") })),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{pr}");
        pr["number"].as_u64().expect("number")
    }
}

/// A store that fails on a read after the repository resolved answers 500 on each compat read
/// and write that needs it, with no store detail; the same request answers again once it heals.
#[tokio::test]
async fn a_store_failing_after_the_repository_resolved_answers_500() {
    let w = seeded().await;
    let number = w.open_pull("dev").await;
    let sha = w.git("site", &["rev-parse", "main"]).await;
    let status = json!({ "state": "success", "context": "ci" });
    for (call, method, uri, body) in [
        (
            "list_protection_rules",
            "GET",
            format!("{R}/branch_protections"),
            None,
        ),
        (
            "get_protection_rule",
            "GET",
            format!("{R}/branch_protections/main"),
            None,
        ),
        (
            "upsert_commit_status",
            "POST",
            format!("{R}/statuses/{sha}"),
            Some(status.clone()),
        ),
        (
            "append_audit",
            "POST",
            format!("{R}/statuses/{sha}"),
            Some(status.clone()),
        ),
        ("list_pull_requests", "GET", format!("{R}/pulls"), None),
        (
            "list_repos",
            "GET",
            "/odata/Repositories('site')".to_owned(),
            None,
        ),
        (
            "get_quota_usage",
            "GET",
            "/odata/Instances".to_owned(),
            None,
        ),
    ] {
        refused_when(&w, call, method, &uri, body.clone()).await;
        let (s, answer) = send(&w.app, method, &uri, body).await;
        assert!(
            s.is_success() || s == StatusCode::NOT_FOUND,
            "{call} healed: {s} {answer}"
        );
    }
    let (s, pr) = send(&w.app, "GET", &format!("{R}/pulls/{number}"), None).await;
    assert_eq!(s, StatusCode::OK, "{pr}");
}

/// A repository whose record cannot be stored leaves no directory behind; one whose audit row
/// cannot be written is not created either.
#[tokio::test]
async fn a_repository_the_store_refuses_leaves_nothing_on_disk() {
    let w = world();
    let dirs = |root: &std::path::Path| walk(root);
    let before = dirs(w.state.repo_root());
    for call in ["create_repo", "append_audit"] {
        w.store.fail(call);
        let (s, answer) = send(
            &w.app,
            "POST",
            "/api/v1/orgs/joinedcontext/repos",
            Some(json!({ "name": "site" })),
        )
        .await;
        w.store.heal();
        assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{call}: {answer}");
        assert_eq!(dirs(w.state.repo_root()), before, "{call} left a directory");
    }
    let (s, _) = send(&w.app, "GET", R, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.join("HEAD").exists() {
                out.push(path);
            } else if path.is_dir() {
                stack.push(path);
            }
        }
    }
    out.sort();
    out
}

/// A write stands when only its bookkeeping fails afterwards: the size read or the quota move.
#[tokio::test]
async fn a_commit_stands_when_its_size_or_quota_cannot_be_recorded() {
    let w = seeded().await;
    for (n, call) in ["get_repo_by_id", "adjust_quota_usage"]
        .into_iter()
        .enumerate()
    {
        w.store.fail(call);
        let (s, answer) = send(
            &w.app,
            "PUT",
            &format!("{R}/contents/f{n}.md"),
            Some(put("x", "more", "main")),
        )
        .await;
        w.store.heal();
        assert_eq!(s, StatusCode::CREATED, "{call}: {answer}");
        let (s, file) = send(&w.app, "GET", &format!("{R}/contents/f{n}.md"), None).await;
        assert_eq!(s, StatusCode::OK, "{call}: {file}");
    }
}

/// A pull request read refreshes its state even when the refreshed record cannot be stored.
#[tokio::test]
async fn a_pull_request_reads_when_its_refresh_cannot_be_stored() {
    let w = seeded().await;
    let number = w.open_pull("dev").await;
    let before = w.git("site", &["rev-parse", "dev"]).await;
    w.git("site", &["update-ref", "refs/heads/dev", "main"])
        .await;
    w.store.fail("update_pull_request");
    let (s, pr) = send(&w.app, "GET", &format!("{R}/pulls/{number}"), None).await;
    w.store.heal();
    assert_eq!(s, StatusCode::OK, "{pr}");
    assert_ne!(pr["head"]["sha"].as_str(), Some(before.as_str()), "{pr}");
}

/// A combined status of only states the forge does not count as green reads `pending`.
#[tokio::test]
async fn a_combined_status_of_pending_checks_reads_pending() {
    let w = seeded().await;
    let sha = w.git("site", &["rev-parse", "main"]).await;
    let (s, saved) = send(
        &w.app,
        "POST",
        &format!("{R}/statuses/{sha}"),
        Some(json!({ "state": "pending", "context": "ci" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{saved}");
    let (s, combined) = send(&w.app, "GET", &format!("{R}/commits/{sha}/status"), None).await;
    assert_eq!(s, StatusCode::OK, "{combined}");
    assert_eq!(combined["state"], "pending", "{combined}");
}

/// A rename to a free name moves the repository: the new name answers, the old one is gone.
#[tokio::test]
async fn a_repository_renamed_to_a_free_name_answers_under_it() {
    let w = seeded().await;
    let (s, repo) = send(&w.app, "PATCH", R, Some(json!({ "name": "portal" }))).await;
    assert_eq!(s, StatusCode::OK, "{repo}");
    assert_eq!(repo["name"], "portal", "{repo}");
    let (s, _) = send(&w.app, "GET", "/api/v1/repos/joinedcontext/portal", None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = send(&w.app, "GET", R, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// Tags a push brought list with their commit, a page at a time.
#[tokio::test]
async fn tags_list_with_their_commit() {
    let w = seeded().await;
    let sha = w.git("site", &["rev-parse", "main"]).await;
    w.git("site", &["tag", "v1", "main"]).await;
    w.git("site", &["tag", "-a", "v2", "-m", "second", "main"])
        .await;
    let (s, tags) = send(&w.app, "GET", &format!("{R}/tags?limit=1&page=2"), None).await;
    assert_eq!(s, StatusCode::OK, "{tags}");
    assert_eq!(tags.as_array().map(Vec::len), Some(1), "{tags}");
    assert_eq!(tags[0]["name"], "v2", "{tags}");
    assert_eq!(tags[0]["commit"]["sha"], sha.as_str(), "{tags}");
}

/// A ref nothing names: the commit list and a bundle of it answer 404, not an empty success.
#[tokio::test]
async fn an_unknown_ref_is_not_found_for_commits_and_bundles() {
    let w = seeded().await;
    for uri in [
        format!("{R}/commits?sha=nowhere"),
        format!("{R}/archive/nowhere.bundle"),
    ] {
        let (s, answer) = send(&w.app, "GET", &uri, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}: {answer}");
    }
}

/// A pull request's branch update takes only `merge`; a merge only a style the forge knows; a
/// base moves to another branch.
#[tokio::test]
async fn a_pull_request_refuses_unknown_styles_and_moves_its_base() {
    let w = seeded().await;
    let number = w.open_pull("dev").await;
    let (s, answer) = send(
        &w.app,
        "POST",
        &format!("{R}/pulls/{number}/update?style=rebase"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    let (s, answer) = send(
        &w.app,
        "POST",
        &format!("{R}/pulls/{number}/merge"),
        Some(json!({ "Do": "octopus" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    w.branch_with_commit("release").await;
    let (s, pr) = send(
        &w.app,
        "PATCH",
        &format!("{R}/pulls/{number}"),
        Some(json!({ "base": "release" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{pr}");
    assert_eq!(pr["base"]["ref"], "release", "{pr}");
    let (s, answer) = send(
        &w.app,
        "PATCH",
        &format!("{R}/pulls/{number}"),
        Some(json!({ "base": "bad..name" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
}

/// The author approves their own pull request when no rule on its base asks for approvals.
#[tokio::test]
async fn an_author_approves_their_own_pull_request_without_a_rule() {
    let w = seeded().await;
    let number = w.open_pull("dev").await;
    let (s, review) = send(
        &w.app,
        "POST",
        &format!("{R}/pulls/{number}/reviews"),
        Some(json!({ "event": "APPROVED", "body": "fine" })),
    )
    .await;
    assert!(s.is_success(), "{review}");
    assert_eq!(review["state"], "APPROVED", "{review}");
}

/// Removing a mirror that is not there succeeds and records nothing.
#[tokio::test]
async fn removing_a_missing_mirror_succeeds_and_audits_nothing() {
    let w = seeded().await;
    let audits = w
        .state
        .store()
        .list_audit(INSTANCE, ORG)
        .await
        .expect("audit")
        .len();
    let (s, answer) = send(&w.app, "DELETE", &format!("{R}/push_mirrors/nowhere"), None).await;
    assert!(s.is_success(), "{s} {answer}");
    let after = w
        .state
        .store()
        .list_audit(INSTANCE, ORG)
        .await
        .expect("audit")
        .len();
    assert_eq!(after, audits);
}

/// A hook's previous secret reference is checked like its secret: a key the forge keeps for
/// another repository is not found, a key-shaped name is taken.
#[tokio::test]
async fn a_hooks_previous_secret_reference_is_checked() {
    let w = seeded().await;
    let hook = |prev: &str| {
        json!({ "type": "gitea", "previous_secret_ref": prev,
        "config": { "url": "https://hooks.city.example/h", "content_type": "json", "secret": "s3cret-value" },
        "events": ["push"], "active": true })
    };
    let other = format!("repo:{}:hook", RepoId::new());
    let (s, answer) = send(&w.app, "POST", &format!("{R}/hooks"), Some(hook(&other))).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    let (s, answer) = send(
        &w.app,
        "POST",
        &format!("{R}/hooks"),
        Some(hook("rotated-hook-key")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{answer}");
}

/// A migration from a repository the forge does not hold is not found; one past the storage
/// quota is refused naming it.
#[tokio::test]
async fn a_migration_refuses_a_missing_source_and_the_storage_quota() {
    let w = seeded().await;
    let migrate = |from: &str| {
        json!({ "clone_addr": format!("http://localhost/joinedcontext/{from}.git"),
        "repo_owner": "joinedcontext", "repo_name": "copy" })
    };
    let (s, answer) = send(
        &w.app,
        "POST",
        "/api/v1/repos/migrate",
        Some(migrate("nowhere")),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    let mut instance = w.state.instance_for(INSTANCE, ORG).await.expect("instance");
    let usage = w
        .state
        .store()
        .get_quota_usage(INSTANCE, ORG)
        .await
        .expect("usage");
    instance.max_storage_bytes = usage.total_bytes() + 1;
    w.state
        .store()
        .upsert_instance(instance)
        .await
        .expect("instance stored");
    let (s, answer) = send(
        &w.app,
        "POST",
        "/api/v1/repos/migrate",
        Some(migrate("site")),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{answer}");
    assert!(
        answer["message"]
            .as_str()
            .is_some_and(|m| m.contains("storage_bytes")),
        "{answer}"
    );
}

/// The OData `Allowed` function takes its arguments in either case and reads `Read` of the
/// instance when none is named.
#[tokio::test]
async fn the_allowed_function_takes_lower_case_arguments() {
    let w = seeded().await;
    let (s, dec) = send(
        &w.app,
        "GET",
        "/odata/Allowed(operation='Write',object='site')",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{dec}");
    assert_eq!(dec["allowed"], true, "{dec}");
    let (s, dec) = send(&w.app, "GET", "/odata/Allowed()", None).await;
    assert_eq!(s, StatusCode::OK, "{dec}");
    assert_eq!(dec["allowed"], true, "{dec}");
}

/// An archive link with no ref and no format links the default branch's zip.
#[tokio::test]
async fn an_archive_link_defaults_to_the_default_branch_zip() {
    let w = seeded().await;
    let (s, link) = send(
        &w.app,
        "GET",
        "/odata/Repositories('site')/Shako.ArchiveLink()",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{link}");
    assert!(
        link["value"]
            .as_str()
            .is_some_and(|v| v.ends_with("/archive/main.zip")),
        "{link}"
    );
}

/// `$allowed` with a key judges that row; an unknown operation is a 400 naming it.
#[tokio::test]
async fn allowed_on_a_set_judges_a_key() {
    let w = seeded().await;
    let id = w.repo("site").await.id;
    let (s, dec) = send(
        &w.app,
        "GET",
        &format!("/odata/Repositories/$allowed(op='update',key='{id}')"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{dec}");
    let (s, answer) = send(
        &w.app,
        "GET",
        "/odata/Repositories/$allowed(op='explode')",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{answer}");
}

/// A path three segments deep names only a pull request's part; anything else is not found,
/// and a pull request number that is not one is a bad request.
#[tokio::test]
async fn a_deep_odata_path_names_only_a_pull_requests_part() {
    let w = seeded().await;
    let (s, answer) = send(
        &w.app,
        "GET",
        "/odata/Repositories('site')/Branches('main')/Files",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    let (s, answer) = send(
        &w.app,
        "POST",
        "/odata/Repositories('site')/PullRequests(x)/Shako.Close",
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{answer}");
}

/// A hook added through OData takes a secret reference the caller names; another repository's
/// generated key is not found, and a secret together with a reference is refused.
#[tokio::test]
async fn an_odata_hook_takes_a_named_secret_reference() {
    let w = seeded().await;
    let hook = |extra: Value| {
        let mut body =
            json!({ "repo": "joinedcontext/site", "Url": "https://hooks.city.example/h" });
        body.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        body
    };
    let (s, answer) = send(
        &w.app,
        "POST",
        "/odata/AddWebhook",
        Some(hook(json!({ "SecretRef": "hook-key" }))),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{answer}");
    assert_eq!(answer["Url"], "https://hooks.city.example/h", "{answer}");
    let foreign = format!("repo:{}:webhook-x", RepoId::new());
    let (s, answer) = send(
        &w.app,
        "POST",
        "/odata/AddWebhook",
        Some(hook(json!({ "SecretRef": foreign }))),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    let (s, answer) = send(
        &w.app,
        "POST",
        "/odata/AddWebhook",
        Some(hook(
            json!({ "SecretRef": "hook-key", "Secret": "s3cret-value" }),
        )),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
}

/// Git over HTTP: dumb HTTP and an unknown service are refused, an anonymous pack request is
/// unauthorized.
#[tokio::test]
async fn git_http_refuses_dumb_http_unknown_services_and_the_anonymous() {
    let w = seeded().await;
    for uri in [
        "/joinedcontext/site/info/refs",
        "/joinedcontext/site/info/refs?service=git-archive",
    ] {
        let (s, answer) = send(&w.app, "GET", uri, None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{uri}: {answer}");
    }
    for uri in [
        "/joinedcontext/site/git-upload-pack",
        "/joinedcontext/site/git-receive-pack",
    ] {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .body(Body::empty())
            .expect("request");
        let resp = w.app.clone().oneshot(req).await.expect("answer");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{uri}");
        let req = Request::builder()
            .method("POST")
            .uri(uri.replace("site", "nowhere"))
            .header("authorization", format!("token {ADMIN}"))
            .body(Body::empty())
            .expect("request");
        let resp = w.app.clone().oneshot(req).await.expect("answer");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

/// A hook whose secret cannot be read is not delivered unsigned: the delivery is recorded failed
/// and nothing leaves. A rotated hook signs with its previous secret too.
#[tokio::test]
async fn a_hook_signs_with_both_secrets_and_never_unsigned() {
    let w = seeded().await;
    let (s, hook) = send(&w.app, "POST", &format!("{R}/hooks"), Some(json!({ "type": "gitea",
        "previous_secret_ref": "rotated-hook-key",
        "config": { "url": "https://hooks.city.example/h", "content_type": "json", "secret": "s3cret-value" },
        "events": ["push"], "active": true }))).await;
    assert_eq!(s, StatusCode::CREATED, "{hook}");
    w.secrets
        .put_secret(ORG, "rotated-hook-key", "old-value")
        .await
        .expect("secret");
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/a.md"),
        Some(put("a", "signed", "main")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let sent = w.deliveries.lock().clone();
    let last = sent.last().expect("a delivery left");
    assert!(
        last.iter().any(|(k, _)| k == "X-Gitea-Signature-Previous"),
        "{last:?}"
    );

    *w.secrets.failing.lock() = true;
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/b.md"),
        Some(put("b", "unsigned", "main")),
    )
    .await;
    *w.secrets.failing.lock() = false;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(
        w.deliveries.lock().len(),
        sent.len(),
        "nothing left without a secret"
    );
    let failed = w
        .state
        .store()
        .list_deliveries(INSTANCE, ORG, None)
        .await
        .expect("deliveries");
    assert!(
        failed
            .iter()
            .any(|d| d.status == "failed" && d.response_code.is_none()),
        "{failed:?}"
    );
}

/// A receiver that redirects is refused by the guard: the delivery is recorded failed and the
/// event is done with, never retried.
#[tokio::test]
async fn a_redirecting_receiver_fails_the_delivery_for_good() {
    let w = seeded().await;
    let (s, hook) = send(&w.app, "POST", &format!("{R}/hooks"), Some(json!({ "type": "gitea",
        "config": { "url": "https://hooks.city.example/moved", "content_type": "json", "secret": "s3cret-value" },
        "events": ["push"], "active": true }))).await;
    assert_eq!(s, StatusCode::CREATED, "{hook}");
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/a.md"),
        Some(put("a", "moved", "main")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let deliveries = w
        .state
        .store()
        .list_deliveries(INSTANCE, ORG, None)
        .await
        .expect("deliveries");
    assert!(
        deliveries
            .iter()
            .any(|d| d.status == "failed" && d.response_code.is_none()),
        "{deliveries:?}"
    );
    let pending = w
        .state
        .store()
        .list_undispatched_outbox(INSTANCE, ORG)
        .await
        .expect("outbox");
    assert!(pending.is_empty(), "{pending:?}");
}

/// A mirror whose credential cannot be read is not pushed; its last error says why and that
/// the next push tries again.
#[tokio::test]
async fn a_mirror_whose_credential_cannot_be_read_records_why() {
    let w = seeded().await;
    let (s, mirror) = send(&w.app, "POST", &format!("{R}/push_mirrors"),
        Some(json!({ "remote_address": "https://github.com/joinedcontext-apps/site.git", "sync_on_commit": true }))).await;
    assert_eq!(s, StatusCode::OK, "{mirror}");
    *w.secrets.failing.lock() = true;
    let (s, sync) = send(&w.app, "POST", &format!("{R}/push_mirrors-sync"), None).await;
    *w.secrets.failing.lock() = false;
    assert_eq!(s, StatusCode::OK, "{sync}");
    let (s, list) = send(&w.app, "GET", &format!("{R}/push_mirrors"), None).await;
    assert_eq!(s, StatusCode::OK, "{list}");
    assert!(
        list[0]["last_error"]
            .as_str()
            .is_some_and(|e| e.contains("credential could not be read")),
        "{list}"
    );
}

/// A member with no binding reads no instance row and no repository; a repository's Team is the
/// group a binding names for it, with its highest verb.
#[tokio::test]
async fn sets_answer_only_what_the_caller_may_read() {
    let w = seeded().await;
    let (s, rows) = send_as(&w.app, OUTSIDER, "GET", "/odata/Instances", None).await;
    assert_eq!(s, StatusCode::OK, "{rows}");
    assert_eq!(rows["value"], json!([]), "{rows}");
    let (s, teams) = send(&w.app, "GET", "/odata/Teams", None).await;
    assert_eq!(s, StatusCode::OK, "{teams}");
    let id = w.repo("site").await.id;
    assert_eq!(
        teams["value"][0]["Id"],
        format!("{id}:site-editors"),
        "{teams}"
    );
    assert_eq!(teams["value"][0]["Verb"], "write", "{teams}");
}

/// A key written `Id='…'` names the row like a bare key; a key whose repository part is no
/// repository id names no repository, and the read asks for one.
#[tokio::test]
async fn a_named_key_reads_like_a_bare_one() {
    let w = seeded().await;
    let id = w.repo("site").await.id;
    let (s, row) = send(
        &w.app,
        "GET",
        &format!("/odata/Branches(Id='{id}:main')"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{row}");
    assert_eq!(row["Name"], "main", "{row}");
    let (s, answer) = send(&w.app, "GET", "/odata/Branches('nowhere:main')", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{answer}");
}

/// A keyed segment of a set with no navigation of its own counts the set, as the plain set does.
#[tokio::test]
async fn a_keyed_count_counts_the_set() {
    let w = seeded().await;
    let req = Request::builder()
        .uri("/odata/Webhooks('x')/$count")
        .header("authorization", format!("token {ADMIN}"))
        .body(Body::empty())
        .expect("request");
    let (s, body) = raw(&w.app, req).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, b"0");
}

/// Operations by name: a body that is not an object names no argument, and a set read whose
/// filter does not parse is a bad request.
#[tokio::test]
async fn operations_by_name_refuse_what_they_cannot_serve() {
    let w = seeded().await;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("token {ADMIN}").parse().expect("header"),
    );
    let ctx = shako_api::authenticate_request(&w.state, &headers, "shako").expect("ctx");
    let mount = jc_router::Mount::default();
    let compared = shako_api::exec_odata_operation(
        &w.state,
        &ctx,
        &mount,
        "Compare",
        Some("site"),
        json!(["main"]),
    )
    .await;
    assert!(
        matches!(&compared, Err(ShakoError::BadRequest(m)) if m.contains("Base")),
        "{compared:?}"
    );
    let listed = shako_api::exec_odata_operation(
        &w.state,
        &ctx,
        &mount,
        "jc_forge_repositories_list",
        None,
        json!({ "Filter": "Name eq" }),
    )
    .await;
    assert!(
        matches!(listed, Err(ShakoError::BadRequest(_))),
        "{listed:?}"
    );
}

/// An archived repository refuses a write naming why; reads go on.
#[tokio::test]
async fn an_archived_repository_refuses_writes() {
    let w = seeded().await;
    let (s, repo) = send(&w.app, "PATCH", R, Some(json!({ "archived": true }))).await;
    assert_eq!(s, StatusCode::OK, "{repo}");
    let (s, answer) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/late.md"),
        Some(put("x", "late", "main")),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{answer}");
    assert!(
        answer["message"]
            .as_str()
            .is_some_and(|m| m.contains("archived")),
        "{answer}"
    );
    let (s, _) = send(&w.app, "GET", &format!("{R}/contents/README.md"), None).await;
    assert_eq!(s, StatusCode::OK);
}

/// A file that cannot be read ends its body with the error, not a short success.
#[tokio::test]
async fn a_file_that_cannot_be_read_ends_its_body_with_the_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = tokio::fs::File::open(dir.path())
        .await
        .expect("a directory opens");
    let body = shako_api::stream_file_in_64k_chunks(file);
    assert!(axum::body::to_bytes(body, 1 << 20).await.is_err());
}

/// The outbox: an event no hook takes is not sent to it, a dispatch that cannot claim leaves
/// the write standing and the event pending, and an event that cannot be marked is an error.
#[tokio::test]
async fn the_outbox_sends_only_what_a_hook_takes_and_keeps_what_it_cannot_send() {
    let w = seeded().await;
    let (s, hook) = send(&w.app, "POST", &format!("{R}/hooks"), Some(json!({ "type": "gitea",
        "config": { "url": "https://hooks.city.example/pulls", "content_type": "json", "secret": "s3cret-value" },
        "events": ["pull_request"], "active": true }))).await;
    assert_eq!(s, StatusCode::CREATED, "{hook}");
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/a.md"),
        Some(put("a", "a push", "main")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert!(
        w.deliveries.lock().is_empty(),
        "a push is not a pull_request event"
    );

    w.store.fail("claim_outbox");
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/b.md"),
        Some(put("b", "kept", "main")),
    )
    .await;
    w.store.heal();
    assert_eq!(s, StatusCode::CREATED);
    let pending = w
        .state
        .store()
        .list_undispatched_outbox(INSTANCE, ORG)
        .await
        .expect("outbox");
    assert_eq!(pending.len(), 1, "{pending:?}");

    w.store.fail("mark_outbox_dispatched=false");
    let marked = shako_api::dispatch_pending_outbox(&w.state, ORG).await;
    w.store.heal();
    assert!(matches!(marked, Err(ShakoError::Storage(m)) if m.contains("could not be marked")));
    assert_eq!(
        shako_api::dispatch_pending_outbox(&w.state, ORG)
            .await
            .expect("dispatch"),
        1
    );
}

/// A push mirror's sync goes on past a store that fails around it: not marked due, a repository
/// that cannot be read, a finish that cannot be recorded. Each answers and the mirror stays unsynced.
#[tokio::test]
async fn a_mirror_sync_answers_past_store_failures() {
    let w = seeded().await;
    let (s, mirror) = send(&w.app, "POST", &format!("{R}/push_mirrors"),
        Some(json!({ "remote_address": "https://github.com/joinedcontext-apps/site.git", "sync_on_commit": true }))).await;
    assert_eq!(s, StatusCode::OK, "{mirror}");
    for call in ["mark_mirror_due", "get_repo_by_id", "finish_mirror_sync"] {
        w.store.fail(call);
        let (s, answer) = send(&w.app, "POST", &format!("{R}/push_mirrors-sync"), None).await;
        w.store.heal();
        assert_eq!(s, StatusCode::OK, "{call}: {answer}");
    }
    let (s, list) = send(&w.app, "GET", &format!("{R}/push_mirrors"), None).await;
    assert_eq!(s, StatusCode::OK, "{list}");
    // Each sync ran to the remote: the error recorded is the remote's answer, not the store's.
    assert!(
        !list[0]["last_error"]
            .as_str()
            .unwrap_or_default()
            .contains("internal"),
        "{list}"
    );
    // The drain a write starts stops at a claim it cannot make and leaves the mirror due.
    w.store.fail("claim_mirror_sync");
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/a.md"),
        Some(put("a", "due", "main")),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    w.store.heal();
    assert_eq!(s, StatusCode::CREATED);
}

/// A mirror stored before the API refused non-key secret names still pushes with it.
#[tokio::test]
async fn a_mirror_with_a_legacy_secret_name_still_pushes() {
    let w = seeded().await;
    let (s, mirror) = send(&w.app, "POST", &format!("{R}/push_mirrors"),
        Some(json!({ "remote_address": "https://github.com/joinedcontext-apps/site.git", "sync_on_commit": true }))).await;
    assert_eq!(s, StatusCode::OK, "{mirror}");
    let mut stored = w
        .state
        .store()
        .list_push_mirrors(INSTANCE, ORG, None)
        .await
        .expect("mirrors")
        .remove(0);
    stored.credential_secret_ref = "a pasted value".into();
    w.state
        .store()
        .upsert_push_mirror(stored)
        .await
        .expect("stored");
    let (s, sync) = send(&w.app, "POST", &format!("{R}/push_mirrors-sync"), None).await;
    assert_eq!(s, StatusCode::OK, "{sync}");
    let (s, list) = send(&w.app, "GET", &format!("{R}/push_mirrors"), None).await;
    assert_eq!(s, StatusCode::OK, "{list}");
    // The push ran with the legacy name: what is recorded is the remote's answer, not a credential refusal.
    let error = list[0]["last_error"].as_str().unwrap_or_default();
    assert!(!error.contains("credential"), "{list}");
}

/// A remote's report-status for every command of a receive-pack request: each ref taken.
fn report_status(request: &[u8]) -> Vec<u8> {
    let mut out = pkt("unpack ok\n");
    let mut rest = request;
    while rest.len() >= 4 {
        let len = std::str::from_utf8(&rest[..4])
            .ok()
            .and_then(|h| usize::from_str_radix(h, 16).ok())
            .unwrap_or(0);
        if len < 4 || len > rest.len() {
            break;
        }
        let line = String::from_utf8_lossy(&rest[4..len]);
        let command = line.split('\0').next().unwrap_or_default();
        if let Some(name) = command.split_whitespace().nth(2) {
            out.extend(pkt(&format!("ok {name}\n")));
        }
        rest = &rest[len..];
    }
    out.extend_from_slice(b"0000");
    out
}

fn pkt(line: &str) -> Vec<u8> {
    let mut out = format!("{:04x}", line.len() + 4).into_bytes();
    out.extend_from_slice(line.as_bytes());
    out
}

fn receive_pack(body: Vec<u8>, length: Option<u64>) -> Request<Body> {
    let req = Request::builder()
        .method("POST")
        .uri("/joinedcontext/site/git-receive-pack")
        .header("authorization", format!("token {ADMIN}"))
        .header("content-type", "application/x-git-receive-pack-request");
    let req = match length {
        Some(n) => req.header("content-length", n),
        None => req,
    };
    req.body(Body::from(body)).expect("request")
}

/// A push that deletes a branch needs no pack: the ref goes and its event is recorded; one whose
/// event cannot be recorded still moves the ref and answers the error.
#[tokio::test]
async fn a_push_deleting_a_branch_records_its_event_or_answers_why_not() {
    let w = seeded().await;
    w.branch_with_commit("dev").await;
    w.branch_with_commit("old").await;
    for (branch, failing) in [("dev", false), ("old", true)] {
        let sha = w.git("site", &["rev-parse", branch]).await;
        let zero = "0".repeat(40);
        let mut body = pkt(&format!(
            "{sha} {zero} refs/heads/{branch}\0report-status delete-refs\n"
        ));
        body.extend_from_slice(b"0000");
        if failing {
            w.store.fail("append_outbox");
        }
        let (s, answer) = raw(&w.app, receive_pack(body, None)).await;
        w.store.heal();
        let text = String::from_utf8_lossy(&answer);
        if failing {
            assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{text}");
        } else {
            assert_eq!(s, StatusCode::OK, "{text}");
            assert!(text.contains(&format!("ok refs/heads/{branch}")), "{text}");
        }
        let (s, _) = send(&w.app, "GET", &format!("{R}/branches/{branch}"), None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{branch} is gone");
    }
}

/// A push declared past the instance's push limit is refused in Git's own words before a byte
/// of it is read.
#[tokio::test]
async fn a_push_past_the_push_limit_answers_unpack_error_quota() {
    let w = seeded().await;
    let mut instance = w.state.instance_for(INSTANCE, ORG).await.expect("instance");
    instance.max_push_bytes = 0;
    w.state
        .store()
        .upsert_instance(instance)
        .await
        .expect("stored");
    let declared = 2 * 1024 * 1024;
    let (s, answer) = raw(&w.app, receive_pack(b"0000".to_vec(), Some(declared))).await;
    assert_eq!(s, StatusCode::OK);
    assert!(String::from_utf8_lossy(&answer).contains("unpack error quota"));
}

/// A mirror that syncs on commit is pushed after a write, its result recorded on that mirror
/// alone: a remote that takes the refs clears the error, one that fails records it. A mirror
/// that does not sync on commit waits for a sync. One that names a branch pushes only it.
#[tokio::test]
async fn mirrors_follow_a_write_each_with_its_own_result() {
    let w = seeded().await;
    w.branch_with_commit("dev").await;
    let add = |remote: &str, on_commit: bool| json!({ "remote_address": format!("https://github.com/joinedcontext-apps/{remote}.git"), "sync_on_commit": on_commit });
    for (remote, on_commit) in [("site", true), ("broken", true), ("later", false)] {
        let (s, m) = send(
            &w.app,
            "POST",
            &format!("{R}/push_mirrors"),
            Some(add(remote, on_commit)),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{m}");
    }
    let (s, m) = send(&w.app, "POST", "/odata/AddPushMirror", Some(json!({ "repo": "joinedcontext/site",
        "RemoteAddress": "https://github.com/joinedcontext-apps/main-only.git", "Branch": "main", "SyncOnCommit": true }))).await;
    assert_eq!(s, StatusCode::OK, "{m}");
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/a.md"),
        Some(put("a", "mirrored", "main")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let mut by_remote = std::collections::BTreeMap::new();
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (_, list) = send(&w.app, "GET", &format!("{R}/push_mirrors"), None).await;
        by_remote = list
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| {
                (
                    m["remote_address"].as_str().unwrap_or_default().to_owned(),
                    m.clone(),
                )
            })
            .collect();
        let settled = |r: &str| {
            by_remote.iter().any(|(k, m)| {
                k.contains(r)
                    && (m["last_update"].is_string()
                        || !m["last_error"].as_str().unwrap_or_default().is_empty())
            })
        };
        if settled("/site.git") && settled("/broken.git") && settled("/main-only.git") {
            break;
        }
    }
    let get = |r: &str| {
        by_remote
            .iter()
            .find(|(k, _)| k.contains(r))
            .map(|(_, m)| m.clone())
            .unwrap_or_default()
    };
    let ok = get("/site.git");
    assert!(
        ok["last_update"].is_string() && ok["last_error"] == "",
        "{ok}"
    );
    let main_only = get("/main-only.git");
    assert!(
        main_only["last_update"].is_string() && main_only["last_error"] == "",
        "{main_only}"
    );
    let broken = get("/broken.git");
    assert!(
        broken["last_update"].is_null() && broken["last_error"] != "",
        "{broken}"
    );
    let later = get("/later.git");
    assert!(
        later["last_update"].is_null() && later["last_error"] == "",
        "{later}"
    );
}

/// A hook's shape: `active` defaults to true; a branch filter other than empty or `*`, a type
/// other than gitea, a content type other than json or a method other than POST are refused;
/// the native shape takes a secret reference and answers without Gitea's fields.
#[tokio::test]
async fn hooks_take_only_the_shapes_the_forge_delivers() {
    let w = seeded().await;
    let gitea = |extra: Value| {
        let mut body = json!({ "type": "gitea",
            "config": { "url": "https://hooks.city.example/h", "content_type": "json", "secret": "s3cret-value" } });
        for (k, v) in extra.as_object().expect("object") {
            if k == "http_method" {
                body["config"][k] = v.clone();
            } else {
                body[k] = v.clone();
            }
        }
        body
    };
    let hooks = format!("{R}/hooks");
    for (extra, want) in [
        (json!({}), StatusCode::CREATED),
        (json!({ "branch_filter": "*" }), StatusCode::CREATED),
        (json!({ "branch_filter": "" }), StatusCode::CREATED),
        (json!({ "http_method": "post" }), StatusCode::CREATED),
        (
            json!({ "branch_filter": "main" }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({ "http_method": "GET" }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (json!({ "type": "slack" }), StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let (s, hook) = send(&w.app, "POST", &hooks, Some(gitea(extra.clone()))).await;
        assert_eq!(s, want, "{extra}: {hook}");
        if s == StatusCode::CREATED {
            assert_eq!(hook["active"], true, "{hook}");
            assert_eq!(hook["type"], "gitea", "{hook}");
        }
    }
    let (s, native) = send(
        &w.app,
        "POST",
        &hooks,
        Some(json!({ "url": "https://hooks.city.example/n", "secret_ref": "native-hook-key" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{native}");
    assert!(
        native.get("type").is_none() && native.get("config").is_none(),
        "{native}"
    );
    let (s, answer) = send(&w.app, "POST", &hooks,
        Some(json!({ "type": "gitea", "url": "https://hooks.city.example/n", "secret_ref": "native-hook-key" }))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
}

/// A migration from the forge's own loopback address copies the repository.
#[tokio::test]
async fn a_migration_from_the_loopback_address_copies() {
    let w = seeded().await;
    let (s, copy) = send(&w.app, "POST", "/api/v1/repos/migrate", Some(json!({
        "clone_addr": "http://127.0.0.1/joinedcontext/site.git", "repo_owner": "joinedcontext", "repo_name": "copy" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{copy}");
    let (s, file) = send(
        &w.app,
        "GET",
        "/api/v1/repos/joinedcontext/copy/contents/README.md",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{file}");
}

/// A tree lists one level unless asked for every level, a page at a time, and says when more
/// pages follow.
#[tokio::test]
async fn a_tree_pages_and_recurses_only_when_asked() {
    let w = seeded().await;
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/docs/a.md"),
        Some(put("a", "nested", "main")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let tree = |q: &str| {
        let uri = format!("{R}/git/trees/main?{q}");
        let app = w.app.clone();
        async move { send(&app, "GET", &uri, None).await }
    };
    let paths = |t: &Value| -> Vec<String> {
        t["tree"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["path"].as_str().map(str::to_owned))
            .collect()
    };
    let (s, flat) = tree("").await;
    assert_eq!(s, StatusCode::OK, "{flat}");
    assert_eq!(paths(&flat), ["README.md", "docs"], "{flat}");
    let (_, deep) = tree("recursive=true").await;
    assert!(paths(&deep).contains(&"docs/a.md".to_owned()), "{deep}");
    let (_, deep) = tree("recursive=1").await;
    assert!(paths(&deep).contains(&"docs/a.md".to_owned()), "{deep}");
    let (_, other) = tree("recursive=yes").await;
    assert_eq!(paths(&other), ["README.md", "docs"], "{other}");
    let (_, first) = tree("per_page=1&page=1").await;
    assert_eq!(
        (paths(&first).len(), first["truncated"].clone()),
        (1, json!(true)),
        "{first}"
    );
    let (_, whole) = tree("per_page=2&page=1").await;
    assert_eq!(
        (paths(&whole).len(), whole["truncated"].clone()),
        (2, json!(false)),
        "{whole}"
    );
    let (_, last) = tree("per_page=1&page=2").await;
    assert_eq!(
        (paths(&last).len(), last["truncated"].clone()),
        (1, json!(false)),
        "{last}"
    );
}

/// A commit status names a full lowercase commit id; anything else is refused.
#[tokio::test]
async fn a_commit_status_needs_a_full_lowercase_id() {
    let w = seeded().await;
    let sha = w.git("site", &["rev-parse", "main"]).await;
    for bad in ["g".repeat(40), sha.to_uppercase(), sha[..39].to_owned()] {
        let (s, answer) = send(
            &w.app,
            "POST",
            &format!("{R}/statuses/{bad}"),
            Some(json!({ "state": "success", "context": "ci" })),
        )
        .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {answer}");
    }
}

/// A file written with an author names that author on its commit.
#[tokio::test]
async fn a_written_file_names_its_author() {
    let w = seeded().await;
    let (s, saved) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/b.md"),
        Some(json!({
        "content": STANDARD.encode("b"), "message": "by Grace", "branch": "main",
        "author": { "name": "Grace", "email": "grace@city.example" } })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{saved}");
    let log = w
        .git("site", &["log", "-1", "--format=%an <%ae>", "main"])
        .await;
    assert_eq!(log, "Grace <grace@city.example>");
}

/// A pull request's branch update with the `merge` style brings in its base.
#[tokio::test]
async fn a_pull_request_branch_updates_with_the_merge_style() {
    let w = seeded().await;
    let number = w.open_pull("dev").await;
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/c.md"),
        Some(put("c", "base moves", "main")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, answer) = send(
        &w.app,
        "POST",
        &format!("{R}/pulls/{number}/update?style=merge"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{answer}");
    let merged = w
        .git("site", &["merge-base", "--is-ancestor", "main", "dev"])
        .await;
    assert_eq!(merged, "");
}
