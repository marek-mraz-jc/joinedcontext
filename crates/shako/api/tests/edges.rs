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
    let authz = Arc::new(LocalAuthorizer::new(vec![RoleBinding {
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
    }]));
    let verifier =
        TokenVerifier::new(ShakoSigningKey::generate().expect("key")).with_static_tokens(vec![
            StaticTokenBinding::new(ADMIN, Subject::authenticated("admin", "admin", vec![org])),
        ]);
    let deliveries = Arc::new(Mutex::new(Vec::new()));
    let seen = deliveries.clone();
    let guard = OutboundGuard::new(OutboundPolicy {
        allowed_hosts: Some(vec!["github.com".into(), "hooks.city.example".into()]),
        mock_handler: Some(Arc::new(move |method, url, headers, _body| {
            if method == "GET" && url.ends_with("/info/refs?service=git-receive-pack") {
                let adv = b"001f# service=git-receive-pack\n000000780000000000000000000000000000000000000000 capabilities^{}\0report-status report-status-v2 delete-refs ofs-delta atomic\n0000";
                return Some((200, adv.to_vec()));
            }
            if method == "POST" && url.ends_with("/git-receive-pack") {
                return Some((200, b"000eunpack ok\n0017ok refs/heads/main\n0000".to_vec()));
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
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("token {ADMIN}"));
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
    assert!(answer["message"].as_str().is_some_and(|m| m.contains("storage_bytes")), "{answer}");
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
        let mut body = json!({ "repo": "joinedcontext/site", "Url": "https://hooks.city.example/h" });
        body.as_object_mut().expect("object").extend(extra.as_object().expect("object").clone());
        body
    };
    let (s, answer) = send(&w.app, "POST", "/odata/AddWebhook", Some(hook(json!({ "SecretRef": "hook-key" })))).await;
    assert_eq!(s, StatusCode::OK, "{answer}");
    assert_eq!(answer["Url"], "https://hooks.city.example/h", "{answer}");
    let foreign = format!("repo:{}:webhook-x", RepoId::new());
    let (s, answer) = send(&w.app, "POST", "/odata/AddWebhook", Some(hook(json!({ "SecretRef": foreign })))).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    let (s, answer) = send(&w.app, "POST", "/odata/AddWebhook",
        Some(hook(json!({ "SecretRef": "hook-key", "Secret": "s3cret-value" })))).await;
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
