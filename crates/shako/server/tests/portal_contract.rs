#![forbid(unsafe_code)]
//! The Portal's forge contract: every call `crates/shako/PORTAL-ODATA.md` lists, made by the
//! Portal's own `ShakoClient` over OData and TCP against shako mounted at `/git` the way `jc`
//! mounts it, with real git repositories on disk. Each answer goes through the Portal's own
//! parsing, so a field shako drops or misnames fails here, not on dev.
//!
//! Webhooks are delivered to the Portal's real `/api/v1/webhooks/shako` route, which checks the
//! `X-Shako-Signature` HMAC with the Portal's own code; the test records what it answered. The
//! Gitea-compatible REST surface outside clients use is tested on its own in `e2e.rs`; this file
//! reads it only to look at state the Portal has no call for.

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use jc_router::{JcRouterBuilder, Mount, MountSpec};
use joinedcontext_portal::forge::shako::committed_text;
use joinedcontext_portal::forge::{
    Author, FileDelete, FileWrite, GitError, MergeStyle, ReviewEvent, ShakoClient,
};
use serde_json::{Value, json};
use sha2::Digest as _;
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_server::{ShakoState, registry_router, router};
use shako_store_memory::MemoryStore;
use std::net::SocketAddr;
use std::sync::Arc;

const OWNER: &str = "joinedcontext";
const APPS_OWNER: &str = "joinedcontext-apps";
const PORTAL_TOKEN: &str = "portal-token-0123456789";
const APPS_TOKEN: &str = "apps-token-0123456789";
const WEBHOOK_SECRET: &str = "portal-webhook-secret-value";

const AUTHOR: Author<'static> = Author {
    name: "Alice Steward",
    email: "alice@city.example",
};

/// What the Portal's webhook route answered, one `(event, status)` per delivery.
type Deliveries = Arc<parking_lot::Mutex<Vec<(String, u16)>>>;

struct World {
    _tmp: tempfile::TempDir,
    state: ShakoState,
    /// `http://127.0.0.1:{port}/git/`, what `JC_SHAKO_URL` names in `jc`.
    base: String,
    portal_hook: String,
    deliveries: Deliveries,
}

impl World {
    async fn start() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let org = OrgId::from_u128(10);
        let apps_org = OrgId::from_u128(11);
        let all = vec![
            Action::Admin,
            Action::Write,
            Action::Approve,
            Action::Publish,
        ];
        let authz = Arc::new(LocalAuthorizer::new(vec![
            RoleBinding {
                org_id: org,
                principal: "jc-portal".into(),
                actions: all.clone(),
                project_id: None,
                repo: None,
            },
            RoleBinding {
                org_id: apps_org,
                principal: "jc-apps".into(),
                actions: all,
                project_id: None,
                repo: None,
            },
        ]));
        let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
            .with_static_tokens(vec![
                StaticTokenBinding::new(
                    PORTAL_TOKEN,
                    Subject::authenticated("jc-portal", "jc-portal", vec![org], vec![]),
                ),
                StaticTokenBinding::new(
                    APPS_TOKEN,
                    Subject::authenticated("jc-apps", "jc-apps", vec![apps_org], vec![]),
                ),
            ]);
        let guard = OutboundGuard::new(OutboundPolicy {
            allowed_hosts: Some(vec!["127.0.0.1".into(), "github.com".into()]),
            allow_loopback_for_tests: true,
            // A push mirror's target answers like GitHub would, without leaving the host.
            mock_handler: Some(Arc::new(|method, url, _headers, _body| {
                if !url.starts_with("https://github.com/") {
                    return None;
                }
                if method == "GET" && url.ends_with("/info/refs?service=git-receive-pack") {
                    let adv = b"001f# service=git-receive-pack\n000000780000000000000000000000000000000000000000 capabilities^{}\0report-status report-status-v2 delete-refs ofs-delta atomic\n0000";
                    return Some((200, adv.to_vec()));
                }
                if method == "POST" && url.ends_with("/git-receive-pack") {
                    return Some((200, b"000eunpack ok\n0017ok refs/heads/main\n0000".to_vec()));
                }
                None
            })),
            ..OutboundPolicy::default()
        });
        let state = ShakoState::new(
            InstanceId::from_u128(1),
            org,
            tmp.path().to_path_buf(),
            Arc::new(MemoryStore::new()),
            authz,
            verifier,
            guard,
        );
        state.register_owner_org(OWNER, org);
        state.register_owner_org(APPS_OWNER, apps_org);
        state
            .secret_store
            .put_secret(org, "portal-webhook", WEBHOOK_SECRET);
        state
            .secret_store
            .put_secret(org, "another-webhook", "not-the-portal-secret");

        // The Portal itself, holding only the webhook secret, behind a recorder of its verdicts.
        let deliveries: Deliveries = Arc::default();
        let mut config = joinedcontext_portal::config::Config::for_tests();
        config.shako_webhook_secret = Some(WEBHOOK_SECRET.into());
        let seen = deliveries.clone();
        let portal = joinedcontext_portal::server::app(joinedcontext_portal::state::AppState::new(
            config, None,
        ))
        .layer(middleware::from_fn(
            move |req: Request<Body>, next: Next| {
                let seen = seen.clone();
                async move {
                    let event = req
                        .headers()
                        .get("x-shako-event")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned();
                    let hook = req.uri().path() == "/api/v1/webhooks/shako";
                    let res: Response = next.run(req).await;
                    if hook {
                        seen.lock().push((event, res.status().as_u16()));
                    }
                    res
                }
            },
        ));
        let portal_addr = serve(portal).await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let host = addr.to_string();
        let git_mount = Mount::new("shako", "http", host.clone(), "/git").unwrap();
        let v2_mount = Mount::new("shako-registry", "http", host.clone(), "/v2").unwrap();
        let jc = JcRouterBuilder::new("http", host.clone())
            .mount(
                MountSpec {
                    module: "shako".into(),
                    host: None,
                    path_prefix: Some("/git".into()),
                },
                router(state.clone(), git_mount),
            )
            .mount(
                MountSpec {
                    module: "shako-registry".into(),
                    host: None,
                    path_prefix: Some("/v2".into()),
                },
                registry_router(state.clone(), v2_mount),
            )
            .build()
            .expect("mount table");
        tokio::spawn(async move {
            axum::serve(listener, jc.into_axum_router()).await.unwrap();
        });

        World {
            _tmp: tmp,
            state,
            base: format!("http://{host}/git/"),
            portal_hook: format!("http://{portal_addr}/api/v1/webhooks/shako"),
            deliveries,
        }
    }

    /// The Portal's client as `jc` configures it from the environment (`JC_SHAKO_*`).
    fn client(&self, repo: &str) -> ShakoClient {
        let base = self.base.clone();
        let repo = repo.to_owned();
        ShakoClient::from_env(move |key| match key {
            "JC_SHAKO_URL" => Some(base.clone()),
            "JC_SHAKO_OWNER" => Some(OWNER.into()),
            "JC_SHAKO_REPO" => Some(repo.clone()),
            "JC_SHAKO_TOKEN" => Some(PORTAL_TOKEN.into()),
            "JC_SHAKO_APPS_OWNER" => Some(APPS_OWNER.into()),
            "JC_SHAKO_APPS_TOKEN" => Some(APPS_TOKEN.into()),
            _ => None,
        })
        .expect("config")
        .expect("forge configured")
    }

    async fn raw(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let url = format!("{}{}", self.base, path.trim_start_matches('/'));
        let mut req = reqwest::Client::new()
            .request(method.parse().unwrap(), url)
            .header("authorization", format!("token {PORTAL_TOKEN}"));
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        let text = res.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn manifest(name: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1\nkind: Endpoint\nmetadata:\n  name: {name}\nspec:\n  public: true\n"
    )
}

/// Repository lifecycle, branches, contents, history and protection: what the reconciler's
/// staging and every proposal write read and write (PORTAL-ODATA.md sections 2 and 3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repository_branches_contents_and_history() {
    let w = World::start().await;
    let forge = w.client("config");

    // ensure_repository: created once, found the second time.
    assert!(forge.ensure_repository("Configuration").await.unwrap());
    assert!(!forge.ensure_repository("Configuration").await.unwrap());
    assert_eq!(forge.default_branch().await.unwrap(), "main");
    let main0 = forge.branch_head("main").await.unwrap();
    assert_eq!(main0.len(), 40);
    assert_eq!(forge.commit_state(&main0).await.unwrap(), None);

    // A workspace branch (a slash in its name) is cut, written, read and listed.
    let ws = "workspace/air-v2";
    forge.create_branch(ws, "main").await.unwrap();
    assert_eq!(forge.branch_head(ws).await.unwrap(), main0);
    let air = manifest("air");
    let c1 = forge
        .write_files(
            ws,
            "Add the air Endpoint",
            AUTHOR,
            &[
                ("spaces/air/endpoints/air.yaml".into(), air.clone(), None),
                ("docs/notes.md".into(), "# Notes\n".into(), None),
            ],
        )
        .await
        .unwrap();
    assert_eq!(forge.branch_head(ws).await.unwrap(), c1);
    let file = forge
        .get_file("spaces/air/endpoints/air.yaml", ws)
        .await
        .unwrap()
        .expect("written file");
    assert_eq!(
        file.content,
        committed_text("spaces/air/endpoints/air.yaml", &air)
    );
    assert!(forge.get_file("nope.yaml", ws).await.unwrap().is_none());
    let blobs = forge.list_tree_blobs(ws).await.unwrap();
    assert!(
        blobs
            .iter()
            .any(|(p, sha)| p == "spaces/air/endpoints/air.yaml" && *sha == file.sha),
        "{blobs:?}"
    );
    let listed = forge.list_tree(&c1).await.unwrap();
    assert!(listed.contains(&"docs/notes.md".to_owned()), "{listed:?}");

    // put_file: a replacement names the blob it replaces; a stale blob is a conflict.
    let air2 = manifest("air-two");
    let c2 = forge
        .put_file(&FileWrite {
            path: "spaces/air/endpoints/air.yaml",
            branch: ws,
            message: "Rename",
            content: &air2,
            sha: Some(&file.sha),
            author: AUTHOR,
        })
        .await
        .unwrap();
    assert_eq!(c2.len(), 40);
    let stale = forge
        .put_file(&FileWrite {
            path: "spaces/air/endpoints/air.yaml",
            branch: ws,
            message: "Stale",
            content: &air,
            sha: Some(&file.sha),
            author: AUTHOR,
        })
        .await;
    assert!(
        matches!(stale, Err(GitError::Conflict(_))),
        "a stale sha is a 409: {stale:?}"
    );

    // update_files refuses the whole commit when one blob moved.
    let head_before = forge.branch_head(ws).await.unwrap();
    let readme = forge.get_file("docs/notes.md", ws).await.unwrap().unwrap();
    let refused = forge
        .update_files(
            ws,
            "Two files",
            AUTHOR,
            &[
                ("docs/notes.md".into(), "# New\n".into(), readme.sha.clone()),
                (
                    "spaces/air/endpoints/air.yaml".into(),
                    air.clone(),
                    file.sha.clone(),
                ),
            ],
        )
        .await;
    assert!(matches!(refused, Err(GitError::Conflict(_))), "{refused:?}");
    assert_eq!(forge.branch_head(ws).await.unwrap(), head_before);

    // change_files: an upload without a sha and a delete naming its blob, in one commit.
    let c3 = forge
        .change_files(
            ws,
            "Upload and delete",
            AUTHOR,
            &[("models/air.yaml".into(), "classes: {}\n".into())],
            &[("docs/notes.md".into(), readme.sha.clone())],
        )
        .await
        .unwrap();
    assert!(forge.get_file("docs/notes.md", ws).await.unwrap().is_none());
    assert!(
        forge
            .get_file("models/air.yaml", ws)
            .await
            .unwrap()
            .is_some()
    );

    // delete_file answers the commit it made.
    let model = forge
        .get_file("models/air.yaml", ws)
        .await
        .unwrap()
        .unwrap();
    let c4 = forge
        .delete_file(&FileDelete {
            path: "models/air.yaml",
            branch: ws,
            message: "Drop the model",
            sha: &model.sha,
            author: AUTHOR,
        })
        .await
        .unwrap();
    assert_eq!(c4.len(), 40);
    assert_ne!(c4, c3);

    // History of one path and of the whole repository, with author and first line.
    let history = forge
        .list_commits(ws, "spaces/air/endpoints/air.yaml", 10)
        .await
        .unwrap();
    assert_eq!(history.len(), 2, "{history:?}");
    assert_eq!(history[0].sha, c2);
    assert_eq!(history[0].message, "Rename");
    assert_eq!(history[0].author, AUTHOR.name);
    assert_eq!(history[0].email.as_deref(), Some(AUTHOR.email));
    assert!(!history[0].date.is_empty());
    let all = forge.list_commits(ws, "", 50).await.unwrap();
    assert!(all.len() >= 5, "{all:?}");
    assert!(
        forge
            .commit_message(&c1)
            .await
            .unwrap()
            .starts_with("Add the air Endpoint")
    );

    // Branch protection: main takes no direct write once protected.
    forge.protect_branch("main").await.unwrap();
    let direct = forge
        .put_file(&FileWrite {
            path: "direct.yaml",
            branch: "main",
            message: "Direct",
            content: "a: 1\n",
            sha: None,
            author: AUTHOR,
        })
        .await;
    assert!(
        matches!(direct, Err(GitError::Api { status: 403, .. })),
        "a protected branch refuses a write with 403: {direct:?}"
    );
    let (st, rules) = w
        .raw(
            "GET",
            "api/v1/repos/joinedcontext/config/branch_protections",
            None,
        )
        .await;
    assert_eq!(st, 200);
    assert_eq!(rules[0]["rule_name"], "main", "{rules}");

    // delete_branch: gone, and gone again is no error.
    forge.delete_branch(ws).await.unwrap();
    forge.delete_branch(ws).await.unwrap();
    assert!(matches!(
        forge.branch_head(ws).await,
        Err(GitError::NotFound)
    ));

    // Archive and delete.
    forge.archive_repository().await.unwrap();
    let (st, repo) = w
        .raw("GET", "api/v1/repos/joinedcontext/config", None)
        .await;
    assert_eq!(st, 200);
    assert_eq!(repo["archived"], true, "{repo}");
    forge.delete_repository().await.unwrap();
    forge.delete_repository().await.unwrap();
    assert!(matches!(
        forge.default_branch().await,
        Err(GitError::NotFound)
    ));
}

/// The change lane: open, read, review, update, merge and close merge requests, with the
/// webhooks the Portal's sync waits for (PORTAL-ODATA.md sections 4 and 8).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn merge_requests_reviews_merge_and_webhooks() {
    let w = World::start().await;
    let forge = w.client("config");
    assert!(forge.ensure_repository("Configuration").await.unwrap());
    let (st, hook) = w
        .raw(
            "POST",
            "api/v1/repos/joinedcontext/config/hooks",
            // A hook naming a stored secret delivers shako's native body, signed in
            // X-Shako-Signature: what `jc` registers for the Portal.
            Some(json!({ "url": w.portal_hook, "secret_ref": "portal-webhook" })),
        )
        .await;
    assert_eq!(st, 201, "{hook}");
    forge.protect_branch("main").await.unwrap();

    // Two proposals cut from the same main.
    for (branch, name) in [("portal/a", "a"), ("portal/b", "b")] {
        forge.create_branch(branch, "main").await.unwrap();
        forge
            .write_files(
                branch,
                &format!("Propose {name}"),
                AUTHOR,
                &[(
                    format!("spaces/s/endpoints/{name}.yaml"),
                    manifest(name),
                    None,
                )],
            )
            .await
            .unwrap();
    }
    let main0 = forge.branch_head("main").await.unwrap();
    let a = forge
        .create_pull_request("portal/a", "main", "Add a", "Proposed in the Portal")
        .await
        .unwrap();
    let b = forge
        .create_pull_request("portal/b", "main", "Add b", "Proposed in the Portal")
        .await
        .unwrap();
    assert_eq!(b.number, a.number + 1);
    assert_eq!(a.state, "open");
    assert_eq!(a.head_branch, "portal/a");
    assert_eq!(a.base_branch, "main");
    assert_eq!(a.head_sha, forge.branch_head("portal/a").await.unwrap());
    assert_eq!(a.base_sha, main0);
    assert!(a.same_repository, "head and base repo_id must match");
    assert_eq!(a.author_login, "jc-portal");
    assert!(!a.created_at.is_empty());
    assert!(
        a.url.ends_with(&format!("/pulls/{}", a.number)),
        "{}",
        a.url
    );
    assert!(!a.merged);

    let open = forge.list_pull_requests("open").await.unwrap();
    assert_eq!(open.len(), 2);
    let page = forge.list_pull_requests_page("open", 2, 1).await.unwrap();
    assert_eq!(page.len(), 1);
    let read = forge.pull_request(a.number).await.unwrap();
    assert_eq!(read.title, "Add a");
    assert!(read.body.starts_with("Proposed in the Portal"));
    assert_eq!(read.mergeable, Some(true));
    let files = forge.pull_request_files(a.number).await.unwrap();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].path, "spaces/s/endpoints/a.yaml");
    assert!(files[0].added && !files[0].deleted);

    // A comment on one's own proposal is taken (the Portal's rejection note).
    forge
        .review(a.number, ReviewEvent::Comment, "Looks right")
        .await
        .unwrap();
    assert_eq!(
        forge.review_bodies(a.number).await.unwrap(),
        vec!["Looks right"]
    );

    // Merge a, pinned to the head that was read.
    forge
        .merge(
            a.number,
            MergeStyle::Merge,
            "Merge a\n\nApproved-by: alice",
            Some(&a.head_sha),
        )
        .await
        .unwrap();
    let merged = forge.pull_request(a.number).await.unwrap();
    assert!(merged.merged);
    assert_eq!(merged.state, "closed");
    assert_eq!(merged.merge_commit_sha.len(), 40);
    assert!(!merged.closed_at.is_empty());
    assert_eq!(merged.merge_base, main0);
    assert_eq!(merged.base_ref(), main0);
    assert_eq!(
        forge.branch_head("main").await.unwrap(),
        merged.merge_commit_sha
    );
    assert!(
        forge
            .commit_message(&merged.merge_commit_sha)
            .await
            .unwrap()
            .contains("Approved-by: alice")
    );
    assert_eq!(forge.list_pull_requests("closed").await.unwrap().len(), 1);

    // b is behind main now: a merge pinned to a head nobody read is refused, the update
    // brings main in, and the merge of the new head goes through.
    let stale = forge
        .merge(b.number, MergeStyle::Merge, "Merge b", Some(&main0))
        .await;
    assert!(
        matches!(stale, Err(GitError::Conflict(_))),
        "a moved head is a 409: {stale:?}"
    );
    forge.update_pull_request(b.number).await.unwrap();
    let b2 = forge.pull_request(b.number).await.unwrap();
    assert_ne!(b2.head_sha, b.head_sha);
    // Already up to date is no error for the Portal's caller either.
    let again = forge.update_pull_request(b.number).await;
    assert!(
        again.is_ok() || matches!(again, Err(GitError::Api { status: 500, .. })),
        "{again:?}"
    );
    let c = forge
        .create_pull_request("portal/b", "main", "Duplicate", "")
        .await;
    assert!(
        matches!(c, Err(GitError::Conflict(_))),
        "a second open request for one head is a 409: {c:?}"
    );
    // The approval page squashes (changes.rs approve).
    forge
        .merge(
            b.number,
            MergeStyle::Squash,
            "Add b\n\nApproved-by: bob",
            Some(&b2.head_sha),
        )
        .await
        .unwrap();
    let squashed = forge.pull_request(b.number).await.unwrap();
    assert!(squashed.merged);
    assert_eq!(
        forge.branch_head("main").await.unwrap(),
        squashed.merge_commit_sha
    );
    assert!(
        forge
            .get_file("spaces/s/endpoints/b.yaml", "main")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        forge
            .get_file("spaces/s/endpoints/a.yaml", "main")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        forge
            .commit_message(&squashed.merge_commit_sha)
            .await
            .unwrap()
            .contains("Approved-by: bob")
    );

    // close_pull_request: closed, not merged, is a rejection.
    forge.create_branch("portal/c", "main").await.unwrap();
    forge
        .write_files(
            "portal/c",
            "Propose c",
            AUTHOR,
            &[("c.txt".into(), "c\n".into(), None)],
        )
        .await
        .unwrap();
    let c = forge
        .create_pull_request("portal/c", "main", "Add c", "")
        .await
        .unwrap();
    forge.close_pull_request(c.number).await.unwrap();
    let closed = forge.pull_request(c.number).await.unwrap();
    assert_eq!(closed.state, "closed");
    assert!(!closed.merged);
    assert!(forge.pull_request(9999).await.is_err());

    // The Portal accepted every delivery's signature, and a merged pull request starts a sync.
    let seen = w.deliveries.lock().clone();
    assert!(!seen.is_empty(), "no webhook reached the Portal");
    assert!(seen.iter().all(|(_, st)| *st != 401), "{seen:?}");
    let merges = seen
        .iter()
        .filter(|(ev, st)| ev == "pull_request" && *st == 202)
        .count();
    assert_eq!(merges, 2, "{seen:?}");
}

/// A delivery signed with any other secret is refused by the Portal (red proof of the HMAC), and
/// so is a Gitea-format delivery signed with the right one: the Portal reads `X-Shako-Signature`
/// alone, never `X-Gitea-Signature`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_webhook_signed_with_another_secret_is_refused_by_the_portal() {
    let w = World::start().await;
    let forge = w.client("config");
    assert!(forge.ensure_repository("Configuration").await.unwrap());
    let (st, _) = w
        .raw(
            "POST",
            "api/v1/repos/joinedcontext/config/hooks",
            Some(json!({ "url": w.portal_hook, "secret_ref": "another-webhook" })),
        )
        .await;
    assert_eq!(st, 201);
    let (st, _) = w
        .raw(
            "POST",
            "api/v1/repos/joinedcontext/config/hooks",
            Some(json!({
                "type": "gitea",
                "config": { "url": w.portal_hook, "content_type": "json", "secret": WEBHOOK_SECRET },
                "events": ["push", "pull_request"],
                "active": true,
            })),
        )
        .await;
    assert_eq!(st, 201);
    forge
        .write_files(
            "main",
            "Direct",
            AUTHOR,
            &[("a.txt".into(), "a\n".into(), None)],
        )
        .await
        .unwrap();
    // One push, one delivery per hook: a refusal is not retried. The Gitea-format delivery
    // carries no X-Shako-Event, so it is recorded with no event.
    let mut seen = w.deliveries.lock().clone();
    seen.sort();
    assert_eq!(seen, vec![(String::new(), 401), ("push".to_owned(), 401)]);
}

/// A hook without a secret is refused at creation, and a hook whose stored secret is gone is
/// never delivered: nothing signed with an empty key leaves the forge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unsigned_hook_is_refused_and_a_lost_secret_delivers_nothing() {
    let w = World::start().await;
    let forge = w.client("config");
    assert!(forge.ensure_repository("Configuration").await.unwrap());
    for body in [
        json!({ "type": "gitea", "config": { "url": w.portal_hook, "content_type": "json" } }),
        json!({ "type": "gitea", "config": { "url": w.portal_hook, "content_type": "json", "secret": " " } }),
        json!({ "type": "gitea", "config": { "url": w.portal_hook, "content_type": "form", "secret": "s" } }),
        json!({ "type": "slack", "config": { "url": w.portal_hook, "secret": "s" } }),
        json!({ "type": "gitea", "config": { "url": w.portal_hook, "secret": "s" }, "branch_filter": "release/*" }),
        json!({ "url": w.portal_hook }),
    ] {
        let (st, answer) = w
            .raw(
                "POST",
                "api/v1/repos/joinedcontext/config/hooks",
                Some(body.clone()),
            )
            .await;
        assert_eq!(st, 422, "{body} -> {answer}");
    }
    let (st, _) = w
        .raw(
            "POST",
            "api/v1/repos/joinedcontext/config/hooks",
            Some(json!({ "url": w.portal_hook, "secret_ref": "no-such-secret" })),
        )
        .await;
    assert_eq!(st, 201);
    forge
        .write_files(
            "main",
            "Direct",
            AUTHOR,
            &[("a.txt".into(), "a\n".into(), None)],
        )
        .await
        .unwrap();
    assert!(w.deliveries.lock().is_empty(), "{:?}", w.deliveries.lock());
}

/// Copies and transfers: bundle out, empty repository, push_bundle in, tags, migrate
/// (PORTAL-ODATA.md section 5).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bundle_push_bundle_tags_and_migrate() {
    let w = World::start().await;
    let origin = w.client("origin");
    assert!(origin.ensure_repository("Origin").await.unwrap());
    origin
        .write_files(
            "main",
            "Add a",
            AUTHOR,
            &[("a.yaml".into(), manifest("a"), None)],
        )
        .await
        .unwrap();
    let head = origin.branch_head("main").await.unwrap();
    assert!(origin.list_tags().await.unwrap().is_empty());

    let bundle = origin.bundle("main").await.unwrap();
    assert!(bundle.starts_with(b"# v2 git bundle\n"), "not a v2 bundle");

    let copy = w.client("copy");
    assert!(copy.create_empty_repository("Copy").await.unwrap());
    assert!(!copy.create_empty_repository("Copy").await.unwrap());
    let refused = copy
        .push_bundle(
            &bundle,
            &[
                (head.clone(), "refs/heads/main".into()),
                (head.clone(), "refs/tags/v1".into()),
            ],
        )
        .await
        .unwrap();
    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(copy.branch_head("main").await.unwrap(), head);
    assert_eq!(
        copy.list_tags().await.unwrap(),
        vec![("v1".to_owned(), head.clone())]
    );
    assert_eq!(
        copy.get_file("a.yaml", "main")
            .await
            .unwrap()
            .unwrap()
            .content,
        committed_text("a.yaml", &manifest("a"))
    );

    // A push of a ref that exists already is answered per ref, the pack still unpacked.
    let again = copy
        .push_bundle(&bundle, &[(head.clone(), "refs/heads/main".into())])
        .await;
    match again {
        Ok(refused) => assert_eq!(refused.len(), 1, "{refused:?}"),
        Err(e) => panic!("the pack unpacks, only the ref is refused: {e:?}"),
    }

    let migrated = w.client("migrated");
    assert!(migrated.migrate_repository(&origin).await.unwrap());
    assert!(!migrated.migrate_repository(&origin).await.unwrap());
    assert_eq!(migrated.branch_head("main").await.unwrap(), head);
}

/// Push mirrors (AP-79), whose credential the forge keeps and never echoes. The Portal makes no
/// team or permission call: access follows RoleBindings (PORTAL-ODATA.md section 7).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_mirrors_and_their_credential() {
    let w = World::start().await;
    let app = w.client("helsinki_bikes").for_application("helsinki_bikes");
    assert!(app.ensure_repository("App").await.unwrap());
    assert!(app.push_mirrors().await.unwrap().is_empty());
    app.add_push_mirror(
        "https://github.com/hel-apps/helsinki_bikes.git",
        "x-access-token",
        "gh-secret",
    )
    .await
    .unwrap();
    let mirrors = app.push_mirrors().await.unwrap();
    assert_eq!(mirrors.len(), 1);
    assert_eq!(
        mirrors[0].remote_address,
        "https://github.com/hel-apps/helsinki_bikes.git"
    );
    assert!(!mirrors[0].remote_name.is_empty());
    app.sync_push_mirrors().await.unwrap();
    let (_, listed) = w
        .raw(
            "GET",
            "api/v1/repos/joinedcontext-apps/helsinki_bikes/push_mirrors",
            None,
        )
        .await;
    assert!(!listed.to_string().contains("gh-secret"), "{listed}");
    app.delete_push_mirror(&mirrors[0].remote_name)
        .await
        .unwrap();
    assert!(app.push_mirrors().await.unwrap().is_empty());
}

/// The applications' organization: generic packages and the container registry the App
/// build publishes to (PORTAL-ODATA.md section 6).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generic_packages_and_container_images() {
    let w = World::start().await;
    let apps = w.client("config").applications();
    apps.put_generic_file("bikes", "1.0.0", "dist.zip", b"zip-bytes".to_vec())
        .await
        .unwrap();
    let twice = apps
        .put_generic_file("bikes", "1.0.0", "dist.zip", b"other".to_vec())
        .await;
    assert!(matches!(twice, Err(GitError::Conflict(_))), "{twice:?}");
    assert_eq!(
        apps.get_generic_file("bikes", "1.0.0", "dist.zip", 1024)
            .await
            .unwrap(),
        b"zip-bytes"
    );
    assert!(
        apps.get_generic_file("bikes", "1.0.0", "dist.zip", 3)
            .await
            .is_err()
    );
    assert!(matches!(
        apps.get_generic_file("bikes", "9.9.9", "dist.zip", 1024)
            .await,
        Err(GitError::NotFound)
    ));

    let config = br#"{"architecture":"amd64","os":"linux"}"#.to_vec();
    let layer = b"layer-bytes".to_vec();
    let digest = |b: &[u8]| format!("sha256:{}", hex::encode(sha2::Sha256::digest(b)));
    let (cd, ld) = (digest(&config), digest(&layer));
    let manifest = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": { "mediaType": "application/vnd.oci.image.config.v1+json", "digest": cd, "size": config.len() },
        "layers": [{ "mediaType": "application/vnd.oci.image.layer.v1.tar", "digest": ld, "size": layer.len() }],
    }))
    .unwrap();
    let stored = apps
        .push_image(
            "bikes",
            "1.0.0",
            &[(&cd, &config), (&ld, &layer)],
            "application/vnd.oci.image.manifest.v1+json",
            &manifest,
        )
        .await
        .unwrap();
    assert_eq!(stored, digest(&manifest));
    // A second push finds every blob held and stores the same manifest.
    let again = apps
        .push_image(
            "bikes",
            "1.0.1",
            &[(&cd, &config), (&ld, &layer)],
            "application/vnd.oci.image.manifest.v1+json",
            &manifest,
        )
        .await
        .unwrap();
    assert_eq!(again, stored);

    // The configuration token holds nothing in the applications' organization.
    let foreign = w
        .client("config")
        .get_generic_file("bikes", "1.0.0", "dist.zip", 1024)
        .await;
    assert!(foreign.is_err(), "{foreign:?}");
}

/// What a SyncSource reads from a git origin (`sync/remote.rs`): the upload-pack advertisement
/// and the archive at the repository's page path. shako serves both, but only to a signed-in
/// caller: deny by default holds for `private: false` repositories too, so the Portal's
/// credential-free fetch of a shako origin is refused (a listed gap, PORTAL-ODATA.md).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sync_source_origin_serves_the_advertisement_and_the_page_archive() {
    let w = World::start().await;
    let forge = w.client("blueprints");
    assert!(forge.ensure_repository("Blueprints").await.unwrap());
    forge
        .write_files(
            "main",
            "Add",
            AUTHOR,
            &[("models/air.yaml".into(), "a: 1\n".into(), None)],
        )
        .await
        .unwrap();
    let head = forge.branch_head("main").await.unwrap();
    let (st, _) = w
        .raw(
            "PATCH",
            "api/v1/repos/joinedcontext/blueprints",
            Some(json!({ "private": false })),
        )
        .await;
    assert_eq!(st, 200);

    let refs_url = format!(
        "{}joinedcontext/blueprints.git/info/refs?service=git-upload-pack",
        w.base
    );
    let zip_url = format!("{}joinedcontext/blueprints/archive/{head}.zip", w.base);
    let http = reqwest::Client::new();
    let signed = |url: &str| {
        http.get(url)
            .header("authorization", format!("token {PORTAL_TOKEN}"))
            .send()
    };

    let refs = signed(&refs_url).await.unwrap();
    assert_eq!(refs.status(), 200);
    let text = refs.text().await.unwrap();
    assert!(text.contains(&format!("{head} refs/heads/main")), "{text}");

    let zip = signed(&zip_url).await.unwrap();
    assert_eq!(zip.status(), 200);
    let bytes = zip.bytes().await.unwrap();
    assert!(bytes.starts_with(b"PK\x03\x04"));
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let names: Vec<String> = archive
        .file_names()
        .map(|n| n.expect("entry name").into_owned())
        .collect();
    // One root directory, which the Portal strips (`unpack`, strip_root).
    let root = names[0].split('/').next().unwrap().to_owned();
    assert!(
        names.iter().all(|n| n.starts_with(&format!("{root}/"))),
        "{names:?}"
    );
    assert!(
        names.iter().any(|n| n.ends_with("/models/air.yaml")),
        "{names:?}"
    );

    // Anonymous: refused, public or not.
    for url in [&refs_url, &zip_url] {
        let anon = http.get(url.as_str()).send().await.unwrap();
        assert_eq!(anon.status().as_u16(), 401, "{url}");
    }
}

/// Actions (App builds) are not served yet: the artifact reads the Portal keeps for builds of
/// another forge get the forge's "not found", which the Portal reports as no artifact instead of
/// failing to parse (PORTAL-ODATA.md section 10). The run, dispatch, queue and runner-token calls
/// are gone from the Portal's client: Actions transport replaced by the Portal build lane,
/// jc-app-builds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actions_calls_answer_not_found_until_runners_land() {
    let w = World::start().await;
    let app = w.client("bikes").for_application("bikes");
    assert!(app.ensure_repository("App").await.unwrap());
    assert!(matches!(
        app.artifacts_named("dist").await,
        Err(GitError::NotFound)
    ));
    assert!(matches!(
        app.download_artifact(1, 1024).await,
        Err(GitError::NotFound)
    ));
    let _ = &w.state;
}
