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
use shako_core::store::{SecretStore, StoreFuture};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, Principal, RepoId,
    RoleBinding, ShakoError, ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
    WebhookId,
};
use std::sync::Arc;
use tower::ServiceExt;

const ADMIN: &str = "admin-token";
const OUTSIDER: &str = "outsider-token";
const R: &str = "/api/v1/repos/joinedcontext/site";

mod support;
use support::fault_store::{FaultStore, refused};

/// A secret store whose reads fail while `failing` is set; writes are kept.
#[derive(Default)]
struct FaultSecrets {
    inner: shako_core::store::MemorySecretStore,
    failing: Mutex<bool>,
    /// Reads never answer while set: a secret store that hangs.
    stalled: Mutex<bool>,
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
        if *self.stalled.lock() {
            return Box::pin(std::future::pending());
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

/// The headers of each hook delivery, in order.
type Deliveries = Arc<Mutex<Vec<Vec<(String, String)>>>>;

struct World {
    app: Router,
    state: ShakoState,
    store: Arc<FaultStore>,
    secrets: Arc<FaultSecrets>,
    /// The headers of each hook delivery the outbound guard let out.
    deliveries: Deliveries,
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
    let deliveries: Deliveries = Arc::new(Mutex::new(Vec::new()));
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
        let out = jc_secret::spawn::command("git")
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

/// A compaction that fails after a write leaves the write standing: the size is measured again
/// and stored, the failure only logged.
#[tokio::test]
async fn a_write_whose_compaction_fails_still_records_its_size() {
    let w = seeded().await;
    let mut repo = w.repo("site").await;
    // Enough packs to be due, none of them a pack: the compaction cannot read them.
    let broken = tempfile::tempdir().expect("dir");
    let packs = broken.path().join("objects/pack");
    std::fs::create_dir_all(&packs).expect("pack dir");
    for n in 0..shako_git::AUTO_COMPACT_PACK_THRESHOLD {
        std::fs::write(packs.join(format!("pack-{n}.pack")), b"0123456789").expect("pack");
        std::fs::write(packs.join(format!("pack-{n}.idx")), b"0123456789").expect("idx");
    }
    assert!(shako_git::compaction_due(broken.path()));
    shako_api::service::settle_after_write(&w.state, &mut repo, broken.path()).await;
    let expected = 20 * shako_git::AUTO_COMPACT_PACK_THRESHOLD as u64;
    assert_eq!(repo.size_bytes, expected);
    assert_eq!(w.repo("site").await.size_bytes, expected);
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

/// A merge asked to delete its head branch merges even when the branch cannot be deleted (its
/// ref is locked): the branch stays and the pull request reads merged.
#[tokio::test]
async fn a_merge_stands_when_its_head_branch_cannot_be_deleted() {
    let w = seeded().await;
    let number = w.open_pull("dev").await;
    let lock = w.disk("site").await.join("refs/heads/dev.lock");
    std::fs::write(&lock, b"").expect("lock the branch");
    let (s, answer) = send(
        &w.app,
        "POST",
        &format!("{R}/pulls/{number}/merge"),
        Some(json!({ "Do": "merge", "delete_branch_after_merge": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{answer}");
    std::fs::remove_file(&lock).expect("unlock");
    let head = w
        .git("site", &["rev-parse", "--verify", "refs/heads/dev"])
        .await;
    assert_eq!(head.len(), 40, "the branch stays: {head}");
    let (s, pr) = send(&w.app, "GET", &format!("{R}/pulls/{number}"), None).await;
    assert_eq!(s, StatusCode::OK, "{pr}");
    assert_eq!(pr["merged"], true, "{pr}");
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
    // A key the set does not hold, or one of the wrong shape, is judged, not an error.
    for (path, reason) in [
        (
            "/odata/Repositories/$allowed(op='update',key='no-such-repository')",
            "served read only",
        ),
        (
            "/odata/PullRequests/$allowed(op='read',key='not-a-number')",
            "has no such entity",
        ),
    ] {
        let (s, dec) = send(&w.app, "GET", path, None).await;
        assert_eq!(s, StatusCode::OK, "{path}: {dec}");
        assert_eq!(dec["allowed"], false, "{path}: {dec}");
        assert!(
            dec["reason"].as_str().is_some_and(|r| r.contains(reason)),
            "{path}: {dec}"
        );
    }
    // A set the service does not serve is a 404 naming it, whatever is asked of it.
    for path in [
        "/odata/NoSuchSet/$allowed(op='read')",
        "/odata/NoSuchSet/$count",
        "/odata/NoSuchSet",
    ] {
        let (s, answer) = send(&w.app, "GET", path, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{path}: {answer}");
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("unknown OData entity set 'NoSuchSet'")),
            "{path}: {answer}"
        );
    }
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

/// A hook stored before the API refused non-key secret names still signs with it, and is
/// delivered beside the hook registered since.
#[tokio::test]
async fn a_hook_with_a_legacy_secret_name_still_signs() {
    let w = seeded().await;
    let (s, hook) = send(&w.app, "POST", &format!("{R}/hooks"), Some(json!({ "type": "gitea",
        "config": { "url": "https://hooks.city.example/h", "content_type": "json", "secret": "s3cret-value" },
        "events": ["push"], "active": true }))).await;
    assert_eq!(s, StatusCode::CREATED, "{hook}");
    let repo = w.repo("site").await;
    let stored = w
        .state
        .store()
        .list_webhooks(INSTANCE, ORG, Some(repo.id))
        .await
        .expect("hooks")
        .remove(0);
    for (secret_ref, previous) in [
        ("a pasted value", None),
        ("legacy-hook-key", Some("an old pasted value")),
    ] {
        w.state
            .store()
            .create_webhook(WebhookRecord {
                id: WebhookId::new(),
                secret_ref: secret_ref.into(),
                previous_secret_ref: previous.map(str::to_owned),
                ..stored.clone()
            })
            .await
            .expect("legacy hook");
        w.secrets
            .put_secret(ORG, secret_ref, "legacy-value")
            .await
            .expect("secret");
    }
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/a.md"),
        Some(put("a", "signed", "main")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let sent = w.deliveries.lock().clone();
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert!(
        sent.iter().all(|h| h
            .iter()
            .any(|(k, v)| k == "X-Gitea-Signature" && !v.is_empty())),
        "{sent:?}"
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

/// A sync now whose remote never answers stops waiting after its 10 s and answers busy; the
/// sync runs on in the background until its 10 minutes are up. Its late result is recorded on
/// no mirror removed meanwhile.
#[tokio::test]
async fn a_sync_that_never_ends_answers_busy_and_writes_back_no_removed_mirror() {
    let w = seeded().await;
    let (s, mirror) = send(&w.app, "POST", &format!("{R}/push_mirrors"),
        Some(json!({ "remote_address": "https://github.com/joinedcontext-apps/site.git", "sync_on_commit": true }))).await;
    assert_eq!(s, StatusCode::OK, "{mirror}");
    let name = mirror["remote_name"]
        .as_str()
        .expect("remote name")
        .to_owned();
    *w.secrets.stalled.lock() = true;
    tokio::time::pause();
    let (s, answer) = send(&w.app, "POST", &format!("{R}/push_mirrors-sync"), None).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{answer}");
    assert!(
        answer.to_string().contains("runs longer than 10 s"),
        "{answer}"
    );
    let (s, answer) = send(&w.app, "DELETE", &format!("{R}/push_mirrors/{name}"), None).await;
    assert!(s.is_success(), "{answer}");
    tokio::time::sleep(std::time::Duration::from_secs(11 * 60)).await;
    *w.secrets.stalled.lock() = false;
    let (s, list) = send(&w.app, "GET", &format!("{R}/push_mirrors"), None).await;
    assert_eq!(s, StatusCode::OK, "{list}");
    assert_eq!(
        list,
        json!([]),
        "a late result brings no removed mirror back"
    );
}

/// A drain that never ends records that the sync took longer than 10 minutes; a third drain
/// while two run starts none.
#[tokio::test]
async fn a_drain_past_its_limit_records_why_and_a_third_starts_none() {
    let w = seeded().await;
    let (s, mirror) = send(&w.app, "POST", &format!("{R}/push_mirrors"),
        Some(json!({ "remote_address": "https://github.com/joinedcontext-apps/site.git", "sync_on_commit": true }))).await;
    assert_eq!(s, StatusCode::OK, "{mirror}");
    *w.secrets.stalled.lock() = true;
    tokio::time::pause();
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/a.md"),
        Some(put("a", "due", "main")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    // The write's drain holds one slot; a second takes the other; a third finds none.
    for _ in 0..2 {
        shako_api::drain_mirrors(&w.state, INSTANCE, ORG);
    }
    tokio::time::sleep(std::time::Duration::from_secs(11 * 60)).await;
    *w.secrets.stalled.lock() = false;
    let (s, list) = send(&w.app, "GET", &format!("{R}/push_mirrors"), None).await;
    assert_eq!(s, StatusCode::OK, "{list}");
    assert!(
        list[0]["last_error"]
            .as_str()
            .is_some_and(|e| e.contains("longer than 10 minutes")),
        "{list}"
    );
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

/// A table filter the OData engine does not serve (`tolower`) is a 501 naming it, in the plain
/// read, `$count`, `$export` and a set read by name.
#[tokio::test]
async fn an_unserved_filter_function_is_not_implemented_everywhere() {
    let w = seeded().await;
    let f = "$filter=tolower(Name)%20eq%20'site'";
    for uri in [
        format!("/odata/Repositories/$count?{f}"),
        format!("/odata/Repositories/$export?{f}"),
    ] {
        let req = Request::builder()
            .uri(&uri)
            .header("authorization", format!("token {ADMIN}"))
            .body(Body::empty())
            .expect("request");
        let (s, body) = raw(&w.app, req).await;
        assert_eq!(
            s,
            StatusCode::NOT_IMPLEMENTED,
            "{uri}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("token {ADMIN}").parse().expect("header"),
    );
    let ctx = shako_api::authenticate_request(&w.state, &headers, "shako").expect("ctx");
    let listed = shako_api::exec_odata_operation(
        &w.state,
        &ctx,
        &jc_router::Mount::default(),
        "jc_forge_repositories_list",
        None,
        json!({ "Filter": "tolower(Name) eq 'site'" }),
    )
    .await;
    assert!(
        matches!(listed, Err(ShakoError::NotImplemented(_))),
        "{listed:?}"
    );
}

/// A key that names nothing is a 404 for a single read; a Compare without its head, an unknown
/// hook's redelivery and an expansion that does not decode are refused naming why.
#[tokio::test]
async fn odata_reads_refuse_keys_and_arguments_that_name_nothing() {
    let w = seeded().await;
    let (s, answer) = send(
        &w.app,
        "GET",
        &format!("/odata/Webhooks('{}')", WebhookId::new()),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    let (s, answer) = send(
        &w.app,
        "GET",
        "/odata/Repositories('site')/Shako.Compare(Base='main')",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{answer}");
    assert!(answer.to_string().contains("Head"), "{answer}");
    let (s, answer) = send(
        &w.app,
        "POST",
        "/odata/RedeliverWebhook",
        Some(json!({ "Id": WebhookId::new().to_string() })),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    let (s, answer) = send(&w.app, "GET", "/odata/PullRequests?$expand=%FF", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{answer}");
    let (s, answer) = send(
        &w.app,
        "POST",
        "/odata/Repositories('site')/Branches('main')/Files",
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
}

/// Operations by name take their defaults: `Allowed` reads `Read`, `Merge` merges, a mirror
/// takes a named secret reference, a commit status without a context is `default`.
#[tokio::test]
async fn operations_by_name_take_their_defaults() {
    let w = seeded().await;
    let (s, dec) = send(&w.app, "POST", "/odata/Allowed", Some(json!({}))).await;
    assert_eq!(s, StatusCode::OK, "{dec}");
    assert_eq!(dec["allowed"], true, "{dec}");
    let number = w.open_pull("dev").await;
    let (s, merged) = send(
        &w.app,
        "POST",
        "/odata/Merge",
        Some(json!({ "repo": "joinedcontext/site", "Number": number })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{merged}");
    let on_main = w
        .git("site", &["merge-base", "--is-ancestor", "dev", "main"])
        .await;
    assert_eq!(on_main, "");
    let (s, mirror) = send(&w.app, "POST", "/odata/AddPushMirror", Some(json!({ "repo": "joinedcontext/site",
        "RemoteAddress": "https://github.com/joinedcontext-apps/site.git", "SecretRef": "mirror-key" }))).await;
    assert_eq!(s, StatusCode::OK, "{mirror}");
    let stored = w
        .state
        .store()
        .list_push_mirrors(INSTANCE, ORG, None)
        .await
        .expect("mirrors");
    assert_eq!(stored[0].credential_secret_ref, "mirror-key");
    let sha = w.git("site", &["rev-parse", "main"]).await;
    let (s, status) = send(
        &w.app,
        "POST",
        &format!("{R}/statuses/{sha}"),
        Some(json!({ "state": "success" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{status}");
    assert_eq!(status["context"], "default", "{status}");
}

/// A commit read by its repository and full id; a page past 500 commits links the next one.
#[tokio::test]
async fn commits_read_by_id_and_page_past_the_limit() {
    let w = seeded().await;
    let id = w.repo("site").await.id;
    let sha = w.git("site", &["rev-parse", "main"]).await;
    let (s, row) = send(
        &w.app,
        "GET",
        &format!("/odata/Commits('{id}:{sha}')"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{row}");
    assert_eq!(row["Sha"], sha.as_str(), "{row}");
    let mut stream = String::new();
    for n in 0..510 {
        stream.push_str(&format!(
            "commit refs/heads/main\ncommitter Ada <ada@city.example> {} +0000\ndata 2\nc{}\n{}\n",
            1_700_000_000 + n,
            n % 10,
            if n == 0 {
                format!("from {sha}\n")
            } else {
                String::new()
            }
        ));
    }
    let mut child = jc_secret::spawn::command("git")
        .arg("-C")
        .arg(w.disk("site").await)
        .args(["fast-import", "--quiet"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("fast-import");
    std::io::Write::write_all(child.stdin.as_mut().expect("stdin"), stream.as_bytes())
        .expect("stream");
    assert!(child.wait().expect("fast-import ends").success());
    let (s, page) = send(
        &w.app,
        "GET",
        &format!("/odata/Commits?$filter=RepoId%20eq%20'{id}'"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(page["value"].as_array().map(Vec::len), Some(500));
    assert!(
        page["@odata.nextLink"]
            .as_str()
            .is_some_and(|l| l.starts_with("Commits?$skiptoken=")),
        "{}",
        page["@odata.nextLink"]
    );
}

/// A body that fails while it is read is a bad request naming the read.
#[tokio::test]
async fn a_body_that_fails_mid_read_is_a_bad_request() {
    let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = vec![
        Ok(bytes::Bytes::from_static(b"{")),
        Err(std::io::Error::other("client left")),
    ];
    let body = Body::from_stream(futures_util::stream::iter(chunks));
    let read = shako_api::read_body_bounded(body, 1024).await;
    assert!(
        matches!(&read, Err(ShakoError::BadRequest(m)) if m.contains("body read error")),
        "{read:?}"
    );
}

/// A repository whose directory is gone answers an error for Git reads, never an empty success.
#[tokio::test]
async fn a_repository_without_its_directory_answers_errors() {
    let w = seeded().await;
    let disk = w.disk("site").await;
    std::fs::remove_dir_all(&disk).expect("removed");
    let (s, answer) = send(&w.app, "GET", &format!("{R}/tags"), None).await;
    assert!(
        s.is_server_error() || s == StatusCode::NOT_FOUND,
        "{s} {answer}"
    );
    let (s, answer) = send(
        &w.app,
        "GET",
        "/joinedcontext/site/info/refs?service=git-upload-pack",
        None,
    )
    .await;
    assert!(
        s.is_server_error() || s == StatusCode::NOT_FOUND,
        "{s} {answer}"
    );
}

/// Git over HTTP: a negotiation that is not one is refused, one declared past its limit too,
/// and a push whose instance cannot be read answers 500.
#[tokio::test]
async fn git_http_refuses_bad_negotiations_and_answers_store_failures() {
    let w = seeded().await;
    let upload = |body: &'static [u8], length: Option<u64>| {
        let req = Request::builder()
            .method("POST")
            .uri("/joinedcontext/site/git-upload-pack")
            .header("authorization", format!("token {ADMIN}"));
        let req = match length {
            Some(n) => req.header("content-length", n),
            None => req,
        };
        req.body(Body::from(body)).expect("request")
    };
    let (s, _) = raw(&w.app, upload(b"zzzz not a pkt-line", None)).await;
    assert!(s.is_client_error(), "{s}");
    let (s, _) = raw(&w.app, upload(b"0000", Some(1 << 30))).await;
    assert!(s.is_client_error(), "{s}");
    w.store.fail("get_instance");
    let (s, _) = raw(&w.app, receive_pack(b"0000".to_vec(), None)).await;
    w.store.heal();
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
}

/// A pull request whose head branch is gone still reads, with its last known state.
#[tokio::test]
async fn a_pull_request_reads_after_its_head_is_gone() {
    let w = seeded().await;
    let number = w.open_pull("dev").await;
    w.git("site", &["update-ref", "-d", "refs/heads/dev"]).await;
    let (s, pr) = send(&w.app, "GET", &format!("{R}/pulls/{number}"), None).await;
    assert_eq!(s, StatusCode::OK, "{pr}");
    assert_eq!(pr["number"], number, "{pr}");
}

/// An owner or repository name that is not one is not found, not a bad request.
#[tokio::test]
async fn a_malformed_owner_is_not_found() {
    let w = seeded().await;
    let (s, answer) = send(&w.app, "GET", "/api/v1/repos/bad..owner/site", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
}

/// A commit to a branch that does not exist is refused naming the branch.
#[tokio::test]
async fn a_commit_to_a_missing_branch_names_it() {
    let w = seeded().await;
    let (s, answer) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/x.md"),
        Some(put("x", "nowhere", "nowhere")),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    assert!(answer.to_string().contains("nowhere"), "{answer}");
}

/// The same forge with a spool budget of `bytes`.
fn with_spool(mut w: World, bytes: u64) -> World {
    w.state.git_work = shako_api::work::GitWork::new(
        w._root.path().join("small-spool"),
        bytes,
        shako_api::work::MEMORY_BYTES,
    );
    w.app = compat_and_native_rest_router()
        .merge(odata_router())
        .merge(smart_http_router())
        .with_state(w.state.clone());
    w
}

/// Feeds `stream` to `git fast-import` in the bare repository `site`.
async fn fast_import(w: &World, stream: &[u8]) {
    let mut child = jc_secret::spawn::command("git")
        .arg("-C")
        .arg(w.disk("site").await)
        .args(["fast-import", "--quiet"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("fast-import");
    std::io::Write::write_all(child.stdin.as_mut().expect("stdin"), stream).expect("stream");
    assert!(child.wait().expect("fast-import ends").success());
}

/// The authenticated request context of the admin.
fn admin_ctx(w: &World) -> shako_core::RequestCtx {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("token {ADMIN}").parse().expect("header"),
    );
    shako_api::authenticate_request(&w.state, &headers, "shako").expect("ctx")
}

/// A file whose answer is larger than the spool budget is refused on each surface naming
/// `spool_bytes`; a small file of the same repository still reads.
#[tokio::test]
async fn a_file_past_the_spool_budget_is_refused_naming_it() {
    let w = with_spool(seeded().await, 2 << 20);
    let main = w.git("site", &["rev-parse", "main"]).await;
    let big = vec![b'a'; 3 << 20];
    let mut stream = format!("blob\nmark :1\ndata {}\n", big.len()).into_bytes();
    stream.extend_from_slice(&big);
    stream.extend_from_slice(
        format!(
            "\ncommit refs/heads/main\ncommitter Ada <ada@city.example> 1700000000 +0000\ndata 3\nbig\nfrom {main}\nM 100644 :1 big.bin\n"
        )
        .as_bytes(),
    );
    fast_import(&w, &stream).await;
    let id = w.repo("site").await.id;
    for uri in [
        format!("{R}/contents/big.bin"),
        format!("/odata/Repositories('{id}')/File(path='big.bin')"),
    ] {
        let (s, answer) = send(&w.app, "GET", &uri, None).await;
        assert!(!s.is_success(), "{uri}: {s}");
        assert!(
            answer.to_string().contains("spool_bytes"),
            "{uri}: {answer}"
        );
    }
    let (s, file) = send(&w.app, "GET", &format!("{R}/contents/README.md"), None).await;
    assert_eq!(s, StatusCode::OK, "{file}");
}

/// A pull request from a branch with no history in common with its base diffs from the base
/// itself: its files and its OData expansion list what the head adds.
#[tokio::test]
async fn a_pull_request_without_a_merge_base_diffs_from_its_base() {
    let w = seeded().await;
    fast_import(
        &w,
        b"commit refs/heads/orphan\ncommitter Ada <ada@city.example> 1700000000 +0000\ndata 6\norphan\nM 100644 inline o.md\ndata 1\no\n",
    )
    .await;
    let (s, pr) = send(
        &w.app,
        "POST",
        &format!("{R}/pulls"),
        Some(json!({ "head": "orphan", "base": "main", "title": "unrelated" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{pr}");
    let number = pr["number"].as_u64().expect("number");
    let (s, files) = send(&w.app, "GET", &format!("{R}/pulls/{number}/files"), None).await;
    assert_eq!(s, StatusCode::OK, "{files}");
    assert!(
        files
            .as_array()
            .is_some_and(|f| f.iter().any(|f| f["filename"] == "o.md")),
        "{files}"
    );
    let (s, page) = send(&w.app, "GET", "/odata/PullRequests?$expand=Files", None).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    let row = &page["value"][0];
    assert_eq!(row["MergeBase"].as_str().unwrap_or_default(), "", "{row}");
    assert!(
        row["Files"].to_string().contains("o.md"),
        "the expansion diffs from the base: {row}"
    );
}

/// A review names the commit it was given, and the head when it was given none or an empty one.
#[tokio::test]
async fn a_review_names_its_commit_or_the_head() {
    let w = seeded().await;
    let number = w.open_pull("dev").await;
    let head = w.git("site", &["rev-parse", "dev"]).await;
    let first = w.git("site", &["rev-parse", "main"]).await;
    for (given, expected) in [
        (json!(first), first.as_str()),
        (json!(""), head.as_str()),
        (Value::Null, head.as_str()),
    ] {
        let (s, review) = send(
            &w.app,
            "POST",
            &format!("{R}/pulls/{number}/reviews"),
            Some(json!({ "event": "COMMENT", "body": "seen", "commit_id": given })),
        )
        .await;
        assert!(s.is_success(), "{given}: {review}");
        assert_eq!(review["commit_id"], expected, "{given}: {review}");
    }
}

/// A tag protection stored for a repository is part of its ref policy and covers only tags.
#[tokio::test]
async fn a_tag_protection_is_part_of_the_ref_policy() {
    let w = seeded().await;
    let repo = w.repo("site").await;
    w.state
        .store()
        .upsert_tag_protection(TagProtectionRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: INSTANCE,
            org_id: ORG,
            repo_id: repo.id,
            rule: TagProtectionRule {
                name_pattern: "v*".into(),
                whitelist_usernames: vec!["admin".into()],
            },
            created_at: jc_time::TimestampTz::now(),
        })
        .await
        .expect("stored");
    let policy = w
        .state
        .ref_policy_for(INSTANCE, ORG, repo.id)
        .await
        .expect("policy");
    assert_eq!(
        policy
            .matching_tag_rule("v1.0")
            .map(|r| r.name_pattern.as_str()),
        Some("v*")
    );
    assert!(policy.matching_tag_rule("release").is_none());
    assert!(policy.matching_branch_rule("v1.0").is_none());
}

/// A write whose operation has no audit kind is refused before anything is recorded.
#[tokio::test]
async fn a_write_without_an_audit_kind_is_refused() {
    let w = seeded().await;
    let ctx = admin_ctx(&w);
    let audited = shako_api::service::audit(&w.state, &ctx, "NoSuchWrite", "repos/x".into()).await;
    assert!(
        matches!(&audited, Err(ShakoError::Internal(m)) if m == "no audit kind for 'NoSuchWrite'"),
        "{audited:?}"
    );
}

/// A branch name Git cannot hold is not found; a ref naming a missing object is not found; a
/// ref file that is not a ref is a Git error; reads and commits alike.
#[tokio::test]
async fn branches_git_cannot_read_answer_not_found_or_a_git_error() {
    let w = seeded().await;
    let heads = w.disk("site").await.join("refs/heads");
    std::fs::write(heads.join("ghost"), format!("{}\n", "a".repeat(40))).expect("ghost ref");
    std::fs::write(heads.join("junk"), "not a ref\n").expect("junk ref");
    let (s, answer) = send(&w.app, "GET", &format!("{R}/branches/a..b"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    let (s, answer) = send(&w.app, "GET", &format!("{R}/branches/ghost"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    let (s, answer) = send(&w.app, "GET", &format!("{R}/branches/junk"), None).await;
    assert!(s.is_server_error(), "{s} {answer}");
    let (s, answer) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/x.md"),
        Some(put("x", "bad name", "a..b")),
    )
    .await;
    assert!(s.is_client_error(), "{s} {answer}");
    let (s, answer) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/x.md"),
        Some(put("x", "junk", "junk")),
    )
    .await;
    assert!(!s.is_success(), "{s} {answer}");
    let (s, _) = send(&w.app, "GET", &format!("{R}/contents/x.md"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "nothing was written");
}

/// A push waits for the repository lock at most a minute, then answers that another write
/// holds it; the clock is paused, so the test waits no minute.
#[tokio::test]
async fn a_push_behind_a_held_lock_answers_overloaded() {
    let w = seeded().await;
    let id = w.repo("site").await.id;
    let _held = w.state.repo_locks.lock(id).await;
    tokio::time::pause();
    let (s, body) = raw(&w.app, receive_pack(b"0000".to_vec(), None)).await;
    assert_eq!(
        s,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        String::from_utf8_lossy(&body).contains("another write holds the repository"),
        "{}",
        String::from_utf8_lossy(&body)
    );
}

/// A repository deleted and created again under its name while a push waited for the lock is
/// a conflict: the push never lands in the new repository.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_push_into_a_repository_replaced_meanwhile_is_a_conflict() {
    let w = seeded().await;
    let old = w.repo("site").await.id;
    let held = w.state.repo_locks.lock(old).await;
    let push = tokio::spawn({
        let app = w.app.clone();
        async move { raw(&app, receive_pack(b"0000".to_vec(), None)).await }
    });
    // The push spools its four bytes and waits for the lock.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert!(!push.is_finished(), "the push waits for the lock");
    assert!(
        w.state
            .store()
            .delete_repo(INSTANCE, ORG, old)
            .await
            .expect("deleted")
    );
    let (s, created) = send(
        &w.app,
        "POST",
        "/api/v1/orgs/joinedcontext/repos",
        Some(json!({ "name": "site" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    drop(held);
    let (s, body) = push.await.expect("push task");
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        String::from_utf8_lossy(&body).contains("replaced during the push"),
        "{}",
        String::from_utf8_lossy(&body)
    );
}

/// A blob path with no directory above it, or one under a file, is a storage error naming why.
#[tokio::test]
async fn a_blob_that_cannot_be_written_is_a_storage_error() {
    let written = shako_api::service::write_blob_file("/".into(), b"x".to_vec()).await;
    assert!(
        matches!(&written, Err(ShakoError::Storage(m)) if m == "a blob path names a file"),
        "{written:?}"
    );
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("file");
    std::fs::write(&file, b"x").expect("file");
    let written = shako_api::service::write_blob_file(file.join("sub/blob"), b"x".to_vec()).await;
    assert!(
        matches!(&written, Err(ShakoError::Storage(m)) if m.starts_with("blob write failed: ")),
        "{written:?}"
    );
    // A name held by a directory with content: the rename over it fails, and the content stays.
    let occupied = dir.path().join("occupied");
    std::fs::create_dir_all(occupied.join("inside")).expect("occupied");
    let written = shako_api::service::write_blob_file(occupied.clone(), b"x".to_vec()).await;
    assert!(occupied.join("inside").is_dir());
    assert!(
        matches!(&written, Err(ShakoError::Storage(m)) if m.starts_with("blob write failed: ")),
        "{written:?}"
    );
}

/// A commit stands when the refreshed repository record cannot be stored.
#[tokio::test]
async fn a_commit_stands_when_its_refreshed_record_cannot_be_stored() {
    let w = seeded().await;
    w.store.fail("update_repo");
    let (s, answer) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/u.md"),
        Some(put("u", "kept", "main")),
    )
    .await;
    w.store.heal();
    assert_eq!(s, StatusCode::CREATED, "{answer}");
    let (s, file) = send(&w.app, "GET", &format!("{R}/contents/u.md"), None).await;
    assert_eq!(s, StatusCode::OK, "{file}");
}

/// Redelivering a hook whose repository is gone is not found, like an unknown hook.
#[tokio::test]
async fn redelivering_a_hook_of_a_deleted_repository_is_not_found() {
    let w = seeded().await;
    let (s, hook) = send(&w.app, "POST", &format!("{R}/hooks"), Some(json!({ "type": "gitea",
        "config": { "url": "https://hooks.city.example/h", "content_type": "json", "secret": "s3cret-value" },
        "events": ["push"], "active": true }))).await;
    assert_eq!(s, StatusCode::CREATED, "{hook}");
    let repo = w.repo("site").await.id;
    assert!(
        w.state
            .store()
            .delete_repo(INSTANCE, ORG, repo)
            .await
            .expect("deleted")
    );
    let (s, answer) = send(
        &w.app,
        "POST",
        "/odata/RedeliverWebhook",
        Some(json!({ "Id": hook["id"].to_string().trim_matches('"') })),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    assert!(answer.to_string().contains("webhook not found"), "{answer}");
}

/// Generic package files: a second file joins its version, a file past what the storage quota
/// leaves is refused naming package storage, and a file whose blob is gone is not found.
#[tokio::test]
async fn generic_package_files_share_a_version_and_respect_storage() {
    let w = seeded().await;
    let ctx = admin_ctx(&w);
    let publish = |file: &'static str, bytes: Vec<u8>| {
        shako_api::service::publish_generic_file(
            &w.state,
            &ctx,
            "joinedcontext",
            "tool",
            "1.0.0",
            file,
            bytes,
        )
    };
    let a = publish("a.bin", b"aaaa".to_vec())
        .await
        .expect("first file");
    let b = publish("b.bin", b"bbbb".to_vec())
        .await
        .expect("second file");
    assert_eq!(a.version_id, b.version_id, "both files are one version's");

    let mut instance = w.state.instance_for(INSTANCE, ORG).await.expect("instance");
    let usage = w
        .state
        .store()
        .get_quota_usage(INSTANCE, ORG)
        .await
        .expect("usage");
    instance.max_storage_bytes = usage.total_bytes() + 8;
    w.state
        .store()
        .upsert_instance(instance)
        .await
        .expect("instance stored");
    let refused = publish("c.bin", vec![0u8; 64]).await;
    assert!(
        matches!(&refused, Err(ShakoError::QuotaExceeded { resource, limit: 8 }) if resource == "package storage"),
        "{refused:?}"
    );

    std::fs::remove_file(shako_api::service::blob_disk_path(
        &w.state,
        "joinedcontext",
        &a.sha256,
    ))
    .expect("blob removed");
    let read = shako_api::service::read_generic_file(
        &w.state,
        &ctx,
        "joinedcontext",
        "tool",
        "1.0.0",
        "a.bin",
    )
    .await;
    assert!(
        matches!(&read, Err(ShakoError::NotFound(m)) if m == "package blob not found"),
        "{:?}",
        read.err()
    );
    let read = shako_api::service::read_generic_file(
        &w.state,
        &ctx,
        "joinedcontext",
        "tool",
        "1.0.0",
        "b.bin",
    )
    .await;
    assert!(read.is_ok(), "the other file still reads");
}

/// A due mirror of a repository deleted before its drain ran is finished without a push: the
/// drain claims it, finds no repository, and records the sync so nothing stays due.
#[tokio::test]
async fn a_due_mirror_of_a_deleted_repository_is_finished_without_a_push() {
    let w = seeded().await;
    let (s, mirror) = send(&w.app, "POST", &format!("{R}/push_mirrors"),
        Some(json!({ "remote_address": "https://github.com/joinedcontext-apps/site.git", "sync_on_commit": true }))).await;
    assert_eq!(s, StatusCode::OK, "{mirror}");
    w.store.fail("claim_mirror_sync");
    let (s, _) = send(
        &w.app,
        "PUT",
        &format!("{R}/contents/a.md"),
        Some(put("a", "due", "main")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    w.store.heal();
    let repo = w.repo("site").await.id;
    assert!(
        w.state
            .store()
            .delete_repo(INSTANCE, ORG, repo)
            .await
            .expect("deleted")
    );
    shako_api::drain_mirrors(&w.state, INSTANCE, ORG);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let due = w
        .state
        .store()
        .claim_mirror_sync(INSTANCE, ORG, None)
        .await
        .expect("claim")
        .is_some();
    assert!(!due, "nothing stays due");
}

/// A bare repository name names only what the caller may read: a member with no binding gets
/// 404 for a name the admin reads, so a name never confirms a hidden repository.
#[tokio::test]
async fn a_bare_name_names_only_what_the_caller_may_read() {
    let w = seeded().await;
    let (s, row) = send(&w.app, "GET", "/odata/Repositories('site')", None).await;
    assert_eq!(s, StatusCode::OK, "{row}");
    let (s, answer) = send_as(&w.app, OUTSIDER, "GET", "/odata/Repositories('site')", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{answer}");
    assert!(
        answer.to_string().contains("repository not found"),
        "{answer}"
    );
}

/// A default branch change whose HEAD file cannot be written is a 500 naming no path, and the
/// stored default branch stays what it was.
#[tokio::test]
async fn a_default_branch_whose_head_cannot_be_written_is_not_stored() {
    let w = seeded().await;
    w.branch_with_commit("dev").await;
    let head = w.disk("site").await.join("HEAD");
    std::fs::remove_file(&head).expect("HEAD removed");
    std::fs::create_dir(&head).expect("HEAD is a directory");
    let (s, answer) = send(&w.app, "PATCH", R, Some(json!({ "default_branch": "dev" }))).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{answer}");
    assert!(!answer.to_string().contains("/HEAD"), "{answer}");
    assert_eq!(w.repo("site").await.default_branch, "main");
}
