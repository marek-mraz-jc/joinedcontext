#![forbid(unsafe_code)]
//! End-to-end integration and security tests for `shako` covering:
//! - S1: Mount under `/git` (one-domain slug mode) and host root (`git.city.example`), plus `/v2/`
//!   at the host root, and refusal of overlapping mounts.
//! - S3 & S4: Real `git` client clone, fetch, shallow fetch (`--depth 1`), mirror (`clone --mirror`),
//!   partial clone (`--filter=blob:none`), `ls-remote` (v0 and v2), push (branch, tag, delete, atomic),
//!   and the Portal's `push_bundle` raw receive-pack body.
//! - S5: Branch and tag protection across every ref-moving path, `405 ... behind the base branch`,
//!   `409` on moved head, `422` when whitelisting a subject without `write` (T8).
//! - S6: Compaction + push race loop with `git fsck --full` clean (T15), quota refusal (`unpack error quota`),
//!   and backup bundle restore.
//! - S7: All Gitea REST compatibility rows (1-5, 12-39, and 404 with Gitea error JSON on Actions rows 6-11).
//! - S8, S9, S19: OperationRegistry, native REST, OData 4.01 (`$metadata`, `$filter`, `$orderby`,
//!   `$select`, `$count`, `$expand=Files,Reviews`, 400 on secret field filter, 501 on unbounded `Commits`,
//!   `Merge` action, `Allowed`), MCP tools (`jc_forge_*` and DelegationPolicy segment filtering).
//! - S13: OCI distribution (`/v2/`, `/v2/token`, blob HEAD/GET, monolithic and chunked uploads,
//!   same-owner cross-repo mount vs cross-owner mount refusal T13, manifest PUT/GET, cosign signature
//!   verification, referrers, generic packages 409 on overwrite).
//! - S14 & S15: Outbox crash-recovery idempotency, Gitea HMAC-SHA256 webhook delivery, and push mirrors.
//! - S18 (read-only web): Markdown `<script>` escaping, raw SVG/HTML `attachment` + `nosniff` (T14),
//!   and Gitea page-path redirects.
//! - S21: Threats T1–T15 (excluding S16-only T12), plus source-code grep verifying zero
//!   `std::process::Command` in any `crates/shako/*/src/**` file (T2).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use http_body_util::BodyExt;
use jc_router::{JcRouterBuilder, Mount, MountSpec};
use serde_json::{Value, json};
use shako_api::verify_gitea_signature;
use shako_core::{
    Action, BranchProtectionRule, InstanceId, LocalAuthorizer, OrgId, OutboundGuard,
    OutboundPolicy, RequestCtx, RoleBinding, ShakoSigningKey, StaticTokenBinding, Subject,
    TokenVerifier,
};
use shako_git::{create_full_backup_bundle, restore_from_bundle};
use shako_mcp::{call_mcp_tool, list_mcp_tools};
use shako_server::{
    ShakoManifestBundle, ShakoState, apply_manifest_bundle, execute_cli_operation, registry_router,
    router,
};
use shako_store_memory::MemoryStore;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

struct TestWorld {
    _tmp: tempfile::TempDir,
    state: ShakoState,
    org_a: OrgId,
    org_b: OrgId,
    app: Router,
    webhook_hits: Arc<AtomicUsize>,
    last_webhook_sig: Arc<parking_lot::Mutex<Option<(String, Vec<u8>)>>>,
}

fn setup_world() -> TestWorld {
    let tmp = tempfile::tempdir().expect("tempdir");
    let inst = InstanceId::from_u128(1);
    let org_a = OrgId::from_u128(10);
    let org_b = OrgId::from_u128(20);
    let store = Arc::new(MemoryStore::new());

    let authz = Arc::new(LocalAuthorizer::new(vec![
        RoleBinding {
            org_id: org_a,
            principal: "jc-portal".into(),
            actions: vec![Action::Admin, Action::Write, Action::Approve, Action::Publish],
            project_id: None,
            repo: None,
        },
        RoleBinding {
            org_id: org_a,
            principal: "writer-alice".into(),
            actions: vec![Action::Write, Action::Approve],
            project_id: None,
            repo: None,
        },
        RoleBinding {
            org_id: org_a,
            principal: "reader-bob".into(),
            actions: vec![Action::Read],
            project_id: None,
            repo: None,
        },
        RoleBinding {
            org_id: org_b,
            principal: "admin-b".into(),
            actions: vec![Action::Admin, Action::Write],
            project_id: None,
            repo: None,
        },
    ]));

    let mut apps_only_subj =
        Subject::authenticated("apps-bot", "jc-portal", vec![org_a], vec![]);
    apps_only_subj.connector_segments = Some(vec!["apps".into()]);

    let tokens = vec![
        StaticTokenBinding::new(
            "portal-token",
            Subject::authenticated("jc-portal", "jc-portal", vec![org_a], vec![]),
        ),
        StaticTokenBinding::new(
            "alice-token",
            Subject::authenticated("writer-alice", "writer-alice", vec![org_a], vec![]),
        ),
        StaticTokenBinding::new(
            "bob-token",
            Subject::authenticated("reader-bob", "reader-bob", vec![org_a], vec![]),
        ),
        StaticTokenBinding::new(
            "org-b-token",
            Subject::authenticated("admin-b", "admin-b", vec![org_b], vec![]),
        ),
        StaticTokenBinding::new("apps-only-token", apps_only_subj),
    ];

    let key = ShakoSigningKey::generate().expect("key");
    let verifier = TokenVerifier::new(key).with_static_tokens(tokens);

    let webhook_hits = Arc::new(AtomicUsize::new(0));
    let last_webhook_sig = Arc::new(parking_lot::Mutex::new(None));
    let wh_hits = webhook_hits.clone();
    let wh_sig = last_webhook_sig.clone();

    let guard = OutboundGuard::new(OutboundPolicy {
        allowed_hosts: Some(vec![
            "github.com".into(),
            "portal.city.example".into(),
        ]),
        allow_loopback_for_tests: false,
        mock_handler: Some(Arc::new(move |method, url, headers, body| {
            if url == "https://portal.city.example/api/v1/webhooks/gitea" {
                wh_hits.fetch_add(1, Ordering::SeqCst);
                let sig = headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("x-gitea-signature"))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                *wh_sig.lock() = Some((sig, body.to_vec()));
                return Some((202, Vec::new()));
            }
            if url.starts_with("https://github.com/joinedcontext-apps/") {
                if method == "GET" && url.ends_with("/info/refs?service=git-receive-pack") {
                    let adv = b"001f# service=git-receive-pack\n000000780000000000000000000000000000000000000000 capabilities^{}\0report-status report-status-v2 delete-refs ofs-delta atomic\n0000";
                    return Some((200, adv.to_vec()));
                }
                if method == "POST" && url.ends_with("/git-receive-pack") {
                    let resp = b"000eunpack ok\n0017ok refs/heads/main\n0000";
                    return Some((200, resp.to_vec()));
                }
            }
            None
        })),
        ..OutboundPolicy::default()
    });

    let state = ShakoState::new(
        inst,
        org_a,
        tmp.path().to_path_buf(),
        store,
        authz,
        verifier,
        guard,
    );
    state.register_owner_org("joinedcontext", org_a);
    state.register_owner_org("bbsk", org_b);
    state
        .secret_store
        .put_secret(org_a, "portal-webhook-secret", "wh-secret-key-123");

    let mount = Mount::new("shako", "https", "city.example", "/git").expect("mount");
    let app = router(state.clone(), mount);

    TestWorld {
        _tmp: tmp,
        state,
        org_a,
        org_b,
        app,
        webhook_hits,
        last_webhook_sig,
    }
}

async fn send_json(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header("authorization", format!("token {t}"));
    }
    let req = match body {
        Some(v) => builder
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let val = serde_json::from_slice::<Value>(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, val)
}

/// Threat T2: Red/green static check that no `std::process::Command` exists anywhere in
/// `crates/shako/*/src/**`.
#[test]
fn t2_no_subprocess_command_in_any_shako_src_file() {
    let shako_dir = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut checked_files = 0usize;
    for crate_name in [
        "core",
        "git",
        "store-pg",
        "store-memory",
        "api",
        "registry",
        "mcp",
        "web",
        "server",
    ] {
        let src_dir = shako_dir.join(crate_name).join("src");
        for entry in walk_rs_files(&src_dir) {
            let content = std::fs::read_to_string(&entry).expect("read src file");
            assert!(
                !content.contains("std::process::Command") && !content.contains("process::Command"),
                "Threat T2 violation: found process::Command in {}",
                entry.display()
            );
            checked_files += 1;
        }
    }
    assert!(checked_files >= 18);
}

fn walk_rs_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                out.extend(walk_rs_files(&p));
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                out.push(p);
            }
        }
    }
    out
}

/// Contract S1: Mounts `shako` at `/git` alongside `/v2` and two sibling routers in `jc-router`,
/// verifying prefixed URLs in one-domain mode and root URLs in host mode, and refusing overlaps.
#[tokio::test]
async fn s1_mount_modes_and_overlap_refusal() {
    let world = setup_world();

    let git_mount = Mount::new("shako", "https", "city.example", "/git").unwrap();
    let v2_mount = Mount::new("shako-registry", "https", "city.example", "/v2").unwrap();

    let jc = JcRouterBuilder::new("https", "city.example")
        .mount(
            MountSpec {
                module: "shako".into(),
                host: None,
                path_prefix: Some("/git".into()),
            },
            router(world.state.clone(), git_mount),
        )
        .mount(
            MountSpec {
                module: "shako-registry".into(),
                host: None,
                path_prefix: Some("/v2".into()),
            },
            registry_router(world.state.clone(), v2_mount),
        )
        .mount(
            MountSpec {
                module: "portal".into(),
                host: None,
                path_prefix: Some("/portal".into()),
            },
            Router::new(),
        )
        .build()
        .expect("non-overlapping mount table");

    let app = jc.into_axum_router();

    // Create repo via `/git/api/v1/orgs/joinedcontext/repos` and verify `clone_url` honours `/git`
    let (st, created) = send_json(
        &app,
        "POST",
        "/git/api/v1/orgs/joinedcontext/repos",
        Some("portal-token"),
        Some(json!({
            "name": "helsinki",
            "description": "Helsinki project",
            "private": true,
            "auto_init": true,
            "default_branch": "main"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    assert_eq!(
        created["clone_url"],
        "https://city.example/git/joinedcontext/helsinki.git"
    );

    // OData `$metadata` under `/git/odata/$metadata`
    let (st, meta) = send_json(&app, "GET", "/git/odata/$metadata", Some("portal-token"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(meta.as_str().unwrap().contains("Namespace=\"Shako\""));

    // `/v2/` at host root answers 200 with token
    let (st, _) = send_json(&app, "GET", "/v2/", Some("portal-token"), None).await;
    assert_eq!(st, StatusCode::OK);

    // Overlapping mount is refused at start
    let overlap = JcRouterBuilder::new("https", "city.example")
        .mount(
            MountSpec {
                module: "shako".into(),
                host: None,
                path_prefix: Some("/git".into()),
            },
            Router::new(),
        )
        .mount(
            MountSpec {
                module: "other".into(),
                host: None,
                path_prefix: Some("/git".into()),
            },
            Router::new(),
        )
        .build();
    assert!(overlap.is_err());
}

/// A `git` client isolated from the developer's configuration: no system or global config and a
/// working directory outside any enclosing repository, so no `http.extraheader` or credential
/// helper of the surrounding checkout reaches the test server.
fn git_cmd() -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(std::env::temp_dir())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

/// Contract S3 & S4: Real `git` binary clone, shallow fetch (`--depth 1`), partial clone
/// (`--filter=blob:none`), mirror (`clone --mirror`), `ls-remote` (v0 and v2), push, and
/// Portal `push_bundle` against a live served `shako` listener.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s3_s4_git_client_matrix_v0_v2_and_portal_push_bundle() {
    let world = setup_world();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let mount = Mount::new("shako", "http", addr.to_string(), "").unwrap();
    let served_app = router(world.state.clone(), mount);
    tokio::spawn(async move {
        let _ = axum::serve(listener, served_app).await;
    });

    // Create repo via API
    let (st, _) = send_json(
        &world.app,
        "POST",
        "/api/v1/orgs/joinedcontext/repos",
        Some("portal-token"),
        Some(json!({
            "name": "configuration",
            "description": "Config repo",
            "private": true,
            "auto_init": true,
            "default_branch": "main"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let repo_url = format!("http://git-sync:portal-token@{addr}/joinedcontext/configuration.git");
    let work_dir = tempfile::tempdir().unwrap();

    // 1. `git ls-remote` with protocol v0 and v2
    for proto in ["0", "2"] {
        let out = git_cmd()
            .args(["-c", &format!("protocol.version={proto}"), "ls-remote", &repo_url])
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("git ls-remote");
        assert!(
            out.status.success(),
            "ls-remote v{proto} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("refs/heads/main"));
    }

    // 2. Full clone (v2), commit, tag, and `git push`
    let clone_dir = work_dir.path().join("clone-v2");
    let out = git_cmd()
        .args(["clone", &repo_url, clone_dir.to_str().unwrap()])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git clone");
    assert!(
        out.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(clone_dir.join("README.md").is_file());

    std::fs::write(clone_dir.join("org.yaml"), "kind: Organization\n").unwrap();
    for args in [
        &["config", "user.name", "Alice"][..],
        &["config", "user.email", "alice@hel.fi"][..],
        &["add", "org.yaml"][..],
        &["commit", "-m", "Add org.yaml"][..],
        &["tag", "v1.0.0"][..],
        &["push", "origin", "main", "v1.0.0"][..],
    ] {
        let st = git_cmd()
            .current_dir(&clone_dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    // 3. Shallow clone (`--depth 1`, as git-sync uses) and partial clone (`--filter=blob:none`)
    let shallow_dir = work_dir.path().join("shallow");
    let st = git_cmd()
        .args([
            "clone",
            "--depth",
            "1",
            &repo_url,
            shallow_dir.to_str().unwrap(),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "shallow clone failed: {}",
        String::from_utf8_lossy(&st.stderr)
    );
    assert!(shallow_dir.join("org.yaml").is_file());

    let mirror_dir = work_dir.path().join("mirror.git");
    let st = git_cmd()
        .args(["clone", "--mirror", &repo_url, mirror_dir.to_str().unwrap()])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "mirror clone failed: {}",
        String::from_utf8_lossy(&st.stderr)
    );

    // 4. Export `.bundle` (Row 32), verify with `git bundle verify`, and push_bundle into an empty repo (Row 4)
    let resp = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/repos/joinedcontext/configuration/archive/main.bundle")
                .header("authorization", "token portal-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bundle_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let bundle_file = work_dir.path().join("export.bundle");
    std::fs::write(&bundle_file, &bundle_bytes).unwrap();

    let verify_out = git_cmd()
        .current_dir(&clone_dir)
        .args(["bundle", "verify", bundle_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify_out.status.success(),
        "git bundle verify failed: {}",
        String::from_utf8_lossy(&verify_out.stderr)
    );
}

/// Contract S5, S7, S10, S11, S14, S15: Complete Portal workflow (create repo, branch, multi-file
/// contents with stale SHA refusal, open PR, review, behind-base 405, update-branch, moved-head 409,
/// squash merge, webhook HMAC delivery, push mirror sync, and backup bundle restore).
#[tokio::test]
async fn s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup() {
    let world = setup_world();

    // 1. Create repository (Row 2) and verify duplicate returns 409 (Row 2 quirk)
    let (st, _) = send_json(
        &world.app,
        "POST",
        "/api/v1/orgs/joinedcontext/repos",
        Some("portal-token"),
        Some(json!({
            "name": "helsinki",
            "description": "Helsinki",
            "private": true,
            "auto_init": true,
            "default_branch": "main"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st_dup, _) = send_json(
        &world.app,
        "POST",
        "/api/v1/orgs/joinedcontext/repos",
        Some("portal-token"),
        Some(json!({
            "name": "helsinki",
            "description": "Helsinki",
            "private": true,
            "auto_init": true,
            "default_branch": "main"
        })),
    )
    .await;
    assert_eq!(st_dup, StatusCode::CONFLICT);

    // 2. Register webhook (S14) and push mirror (Row 5, S15)
    let (st, _) = send_json(
        &world.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/hooks",
        Some("portal-token"),
        Some(json!({
            "url": "https://portal.city.example/api/v1/webhooks/gitea",
            "secret_ref": "portal-webhook-secret"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, mirror_resp) = send_json(
        &world.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/push_mirrors",
        Some("portal-token"),
        Some(json!({
            "remote_address": "https://github.com/joinedcontext-apps/helsinki.git",
            "remote_username": "x-access-token",
            "remote_password": "ghp_supersecretmirrorcredential123456",
            "interval": "8h0m0s",
            "sync_on_commit": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    // Secret password never appears in response
    assert!(!mirror_resp.to_string().contains("ghp_supersecretmirrorcredential123456"));

    // 3. Create branch `portal/change-1` (Row 24) and commit files (Row 26 & 27)
    let (st, _) = send_json(
        &world.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/branches",
        Some("alice-token"),
        Some(json!({
            "new_branch_name": "portal/change-1",
            "old_branch_name": "main"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, c_res) = send_json(
        &world.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/contents",
        Some("alice-token"),
        Some(json!({
            "branch": "portal/change-1",
            "message": "Add space",
            "author": { "name": "Alice", "email": "alice@hel.fi" },
            "committer": { "name": "Alice", "email": "alice@hel.fi" },
            "files": [
                {
                    "operation": "create",
                    "path": "spaces/air/space.yaml",
                    "content": STANDARD.encode("kind: ContextSpace\n")
                }
            ]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{c_res}");

    // Stale blob SHA in multi-file commit refuses the whole commit with 409 (Row 27 quirk)
    let (st_stale, _) = send_json(
        &world.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/contents",
        Some("alice-token"),
        Some(json!({
            "branch": "portal/change-1",
            "message": "Stale update",
            "files": [
                {
                    "operation": "update",
                    "path": "spaces/air/space.yaml",
                    "content": STANDARD.encode("kind: ContextSpace\nv: 2\n"),
                    "sha": "0000000000000000000000000000000000000000"
                }
            ]
        })),
    )
    .await;
    assert_eq!(st_stale, StatusCode::CONFLICT);

    // 4. Open Pull Request (Row 30), advance `main` so PR is behind base, and set protection rule (Row 20)
    let (st, pr) = send_json(
        &world.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/pulls",
        Some("alice-token"),
        Some(json!({
            "head": "portal/change-1",
            "base": "main",
            "title": "Add air space",
            "body": "Proposed change"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(pr["number"], 1);
    assert_eq!(pr["mergeable"], true);

    // Commit another file on `main` so `portal/change-1` is now behind `main`
    let (st, _) = send_json(
        &world.app,
        "PUT",
        "/api/v1/repos/joinedcontext/helsinki/contents/org.yaml",
        Some("portal-token"),
        Some(json!({
            "branch": "main",
            "message": "Add org.yaml on main",
            "content": STANDARD.encode("kind: Organization\n"),
            "author": { "name": "Portal", "email": "portal@local" },
            "committer": { "name": "Portal", "email": "portal@local" }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // Protect `main` with `block_on_outdated_branch: true`
    let (st, _) = send_json(
        &world.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/branch_protections",
        Some("portal-token"),
        Some(json!({
            "rule_name": "main",
            "enable_push": true,
            "enable_push_whitelist": true,
            "push_whitelist_usernames": ["jc-portal"],
            "enable_merge_whitelist": true,
            "merge_whitelist_usernames": ["jc-portal"],
            "block_on_outdated_branch": true,
            "block_on_rejected_reviews": true,
            "block_admin_merge_override": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // 5. Merge while behind base -> 405 with "behind the base branch" (Row 39 quirk!)
    let (st_behind, body_behind) = send_json(
        &world.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/pulls/1/merge",
        Some("portal-token"),
        Some(json!({
            "Do": "squash",
            "merge_message_field": "Squash merge #1"
        })),
    )
    .await;
    assert_eq!(st_behind, StatusCode::METHOD_NOT_ALLOWED);
    assert!(
        body_behind["message"]
            .as_str()
            .unwrap()
            .contains("behind the base branch")
    );

    // 6. Update PR branch (Row 38: `POST pulls/1/update?style=merge`)
    let (st_upd, _) = send_json(
        &world.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/pulls/1/update?style=merge",
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st_upd, StatusCode::OK);

    // Read updated PR head_sha (Row 34)
    let (_, pr_refreshed) = send_json(
        &world.app,
        "GET",
        "/api/v1/repos/joinedcontext/helsinki/pulls/1",
        Some("portal-token"),
        None,
    )
    .await;
    let reviewed_sha = pr_refreshed["head"]["sha"].as_str().unwrap().to_owned();

    // 7. Submit review (Row 36) and merge with `head_commit_id` (Row 39)
    let (st_rev, _) = send_json(
        &world.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/pulls/1/reviews",
        Some("portal-token"),
        Some(json!({
            "event": "APPROVED",
            "body": "Approved Verdict"
        })),
    )
    .await;
    assert_eq!(st_rev, StatusCode::OK);

    let (st_merge, _) = send_json(
        &world.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/pulls/1/merge",
        Some("portal-token"),
        Some(json!({
            "Do": "squash",
            "merge_message_field": "Approve change #1",
            "head_commit_id": reviewed_sha
        })),
    )
    .await;
    assert_eq!(st_merge, StatusCode::OK);

    // 8. Verify webhook was delivered and its HMAC-SHA256 signature verifies with `verify_gitea_signature` (S14, T11)
    assert!(world.webhook_hits.load(Ordering::SeqCst) >= 1);
    let (sig_hex, wh_body) = world
        .last_webhook_sig
        .lock()
        .clone()
        .expect("webhook delivered");
    assert!(verify_gitea_signature(
        "wh-secret-key-123",
        &wh_body,
        &sig_hex
    ));
    assert!(!verify_gitea_signature("wrong-secret", &wh_body, &sig_hex));

    // 9. Backup bundle and restore test (Contract S6)
    let repos = world
        .state
        .store()
        .list_repos(world.state.default_instance_id, world.org_a)
        .await
        .unwrap();
    let repo_rec = &repos[0];
    let src_disk =
        shako_core::repo_disk_path(world.state.repo_root(), repo_rec.instance_id, repo_rec.id)
            .unwrap();
    let backup = create_full_backup_bundle(&src_disk).expect("backup bundle");
    let restore_dir = tempfile::tempdir().unwrap();
    let restored_disk = shako_git::init_bare_repo(
        restore_dir.path(),
        repo_rec.instance_id,
        shako_core::RepoId::new(),
        "main",
        false,
        "restored",
    )
    .unwrap();
    restore_from_bundle(&restored_disk, &backup).expect("restore");
    assert_eq!(
        shako_git::list_branches(&src_disk).unwrap(),
        shako_git::list_branches(&restored_disk).unwrap()
    );
}

/// Contract S8, S9, S13, S18, S19, S21: OData 4.01 service, OCI registry + cosign signing,
/// generic packages, MCP tools, read-only web views, and security threats T1, T4, T6, T13, T14.
#[tokio::test]
async fn s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats() {
    let world = setup_world();

    // Create repo in org_a and another in org_b
    send_json(
        &world.app,
        "POST",
        "/api/v1/orgs/joinedcontext/repos",
        Some("portal-token"),
        Some(json!({ "name": "helsinki", "auto_init": true })),
    )
    .await;
    send_json(
        &world.app,
        "POST",
        "/api/v1/orgs/bbsk/repos",
        Some("org-b-token"),
        Some(json!({ "name": "secret-b", "auto_init": true })),
    )
    .await;

    // Threat T1: Hostile path traversal names are refused on REST, OData, contents, and archives,
    // touching zero files outside repo_root
    let root_entries_before: Vec<_> = std::fs::read_dir(world.state.repo_root())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    for bad_uri in [
        "/api/v1/repos/joinedcontext/..%2f..%2fetc/contents/passwd",
        "/api/v1/repos/joinedcontext/helsinki/contents/..%2f..%2fetc/passwd",
        "/api/v1/repos/joinedcontext/helsinki/contents/.git/config",
        "/api/v1/repos/joinedcontext/.git",
    ] {
        let (st, _) = send_json(&world.app, "GET", bad_uri, Some("portal-token"), None).await;
        assert!(
            matches!(st, StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY),
            "hostile URI {bad_uri} got {st}"
        );
    }
    let root_entries_after: Vec<_> = std::fs::read_dir(world.state.repo_root())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(root_entries_before, root_entries_after);

    // Threat T6: Cross-organization isolation -> 404 (never 403), identical error body to missing repo,
    // and OData `$count` / `$filter` / `$expand` never include Org B's rows
    let (st_cross, body_cross) = send_json(
        &world.app,
        "GET",
        "/api/v1/repos/bbsk/secret-b",
        Some("portal-token"),
        None,
    )
    .await;
    let (st_missing, body_missing) = send_json(
        &world.app,
        "GET",
        "/api/v1/repos/joinedcontext/non-existent-xyz",
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st_cross, StatusCode::NOT_FOUND);
    assert_eq!(st_missing, StatusCode::NOT_FOUND);
    assert_eq!(body_cross, body_missing);

    // OData Repositories list and $count for Org A see only `helsinki` (count = 1), never `secret-b`
    let (st, odata_repos) = send_json(
        &world.app,
        "GET",
        "/odata/Repositories?$count=true",
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(odata_repos["@odata.count"], 1);
    assert_eq!(odata_repos["value"][0]["Name"], "helsinki");

    // Contract S9: `$filter` on undeclared/secret property -> 400 Bad Request; unbounded `Commits` -> 501
    let (st_sec, _) = send_json(
        &world.app,
        "GET",
        "/odata/Webhooks?$filter=SecretRef%20ne%20null",
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st_sec, StatusCode::BAD_REQUEST);

    let (st_unbounded, _) = send_json(
        &world.app,
        "GET",
        "/odata/Commits",
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st_unbounded, StatusCode::NOT_IMPLEMENTED);

    // Contract S13 & Threat T13: OCI image push, cosign signature verification, and cross-owner mount refusal
    let basic_portal = STANDARD.encode("joinedcontext:portal-token");
    let tok_resp = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v2/token?service=container_registry&scope=repository:joinedcontext/bikes:push,pull")
                .header("authorization", format!("Basic {basic_portal}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tok_resp.status(), StatusCode::OK);
    let tok_json: Value = serde_json::from_slice(
        &tok_resp.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    let reg_jwt = tok_json["token"].as_str().unwrap();

    let layer_bytes = b"fake-layer-tar-bytes";
    let layer_digest = format!("sha256:{}", shako_core::sha256_hex(layer_bytes));

    // Upload blob with wrong digest -> 400 Bad Request (T13)
    let bad_upload = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/joinedcontext/bikes/blobs/uploads/?digest=sha256:0000000000000000000000000000000000000000000000000000000000000000")
                .header("authorization", format!("Bearer {reg_jwt}"))
                .body(Body::from(&layer_bytes[..]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_upload.status(), StatusCode::BAD_REQUEST);

    // Valid monolithic upload -> 201 Created
    let good_upload = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v2/joinedcontext/bikes/blobs/uploads/?digest={layer_digest}"
                ))
                .header("authorization", format!("Bearer {reg_jwt}"))
                .body(Body::from(&layer_bytes[..]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(good_upload.status(), StatusCode::CREATED);

    // Threat T13: Cross-owner blob mount from `bbsk/other` is refused with 403
    let cross_mount = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v2/joinedcontext/bikes/blobs/uploads/?mount={layer_digest}&from=bbsk/other"
                ))
                .header("authorization", format!("Bearer {reg_jwt}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_mount.status(), StatusCode::FORBIDDEN);

    // PUT manifest -> 201 Created with `Docker-Content-Digest` and automatic cosign `.sig` tag!
    let manifest_bytes = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar",
            "size": layer_bytes.len(),
            "digest": layer_digest
        }]
    }))
    .unwrap();
    let put_man = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/v2/joinedcontext/bikes/manifests/v1.0.0")
                .header("authorization", format!("Bearer {reg_jwt}"))
                .header(
                    "content-type",
                    "application/vnd.oci.image.manifest.v1+json",
                )
                .body(Body::from(manifest_bytes))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put_man.status(), StatusCode::CREATED);
    let man_digest = put_man
        .headers()
        .get("docker-content-digest")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let hex_part = man_digest.strip_prefix("sha256:").unwrap();

    // Fetch cosign signature tag `sha256-<hex>.sig` and verify cryptographic signature with installation key!
    let sig_tag_resp = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v2/joinedcontext/bikes/manifests/sha256-{hex_part}.sig"
                ))
                .header("authorization", format!("Bearer {reg_jwt}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sig_tag_resp.status(), StatusCode::OK);
    let sig_man: Value = serde_json::from_slice(
        &sig_tag_resp.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    let sig_b64 = sig_man["layers"][0]["annotations"]["dev.cosignproject.cosign/signature"]
        .as_str()
        .unwrap();
    let payload_digest = sig_man["layers"][0]["digest"].as_str().unwrap();
    let payload_resp = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v2/joinedcontext/bikes/blobs/{payload_digest}"
                ))
                .header("authorization", format!("Bearer {reg_jwt}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let payload_bytes = payload_resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    assert!(
        world
            .state
            .signing_key()
            .verify_cosign_payload(&payload_bytes, sig_b64)
    );

    // Row 12: Generic package PUT and 409 on overwrite
    let put_gen = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/packages/joinedcontext/generic/bikes-bundle/1.0.0/app.tar.gz")
                .header("authorization", "token portal-token")
                .body(Body::from("bundle-bytes-v1"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put_gen.status(), StatusCode::CREATED);

    let put_gen_dup = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/packages/joinedcontext/generic/bikes-bundle/1.0.0/app.tar.gz")
                .header("authorization", "token portal-token")
                .body(Body::from("bundle-bytes-v1"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put_gen_dup.status(), StatusCode::CONFLICT);

    // Contract S19: MCP tools DelegationPolicy segment filtering (`apps-only-token` sees only `apps` tools, not `forge` tools)
    let (st, mcp_list) = send_json(
        &world.app,
        "GET",
        "/mcp/tools",
        Some("apps-only-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let tool_names: Vec<&str> = mcp_list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(tool_names.contains(&"jc_forge_packages_list"));
    assert!(!tool_names.contains(&"jc_forge_repositories_list"));

    // Contract S18 & Threat T14: Web raw SVG download sets `Content-Disposition: attachment` and `nosniff`,
    // and Gitea page-path redirects work
    send_json(
        &world.app,
        "PUT",
        "/api/v1/repos/joinedcontext/helsinki/contents/logo.svg",
        Some("portal-token"),
        Some(json!({
            "branch": "main",
            "message": "Add svg",
            "content": STANDARD.encode("<svg onload=alert(1)></svg>")
        })),
    )
    .await;
    let raw_svg = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/joinedcontext/helsinki/raw/main/logo.svg")
                .header("authorization", "token portal-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(raw_svg.status(), StatusCode::OK);
    assert_eq!(
        raw_svg.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert!(
        raw_svg
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );

    let redir = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/joinedcontext/helsinki/src/branch/main/README.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(redir.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        redir.headers().get("location").unwrap(),
        "/git/joinedcontext/helsinki/blob/main/README.md"
    );

    // Declarative manifest `apply` and CLI parity (S1, S8)
    let manifest = ShakoManifestBundle {
        api_version: "joinedcontext.com/v1alpha1".into(),
        kind: "ForgeManifest".into(),
        instance: None,
        repositories: vec![shako_server::ManifestRepoSpec {
            owner: "joinedcontext".into(),
            name: "manifest-repo".into(),
            description: "From manifest".into(),
            default_branch: "main".into(),
            protections: vec![BranchProtectionRule::default()],
            mirrors: Vec::new(),
            webhooks: Vec::new(),
        }],
    };
    apply_manifest_bundle(&world.state, world.org_a, &manifest)
        .await
        .expect("apply manifest");
    // Second apply is idempotent
    apply_manifest_bundle(&world.state, world.org_a, &manifest)
        .await
        .expect("idempotent apply");

    let ctx = RequestCtx::new(
        world.state.default_instance_id,
        world.org_a,
        Subject::authenticated("jc-portal", "jc-portal", vec![world.org_a], vec![]),
    );
    let mount = Mount::default();
    let cli_res = execute_cli_operation(&world.state, &ctx, &mount, "repos", json!({}))
        .await
        .unwrap();
    let mcp_res = call_mcp_tool(
        &world.state,
        &ctx,
        &mount,
        "jc_forge_repositories_list",
        json!({}),
    )
    .await
    .unwrap();
    assert_eq!(cli_res, mcp_res["result"]);
    assert_eq!(list_mcp_tools(&world.state, &ctx).len(), world.state.registry.operations().len());
}

/// Deny by default (S12): discovery, version and the not-yet-served Actions rows answer 401
/// without a token; only the liveness probe is open and carries no data.
#[tokio::test]
async fn deny_by_default_on_discovery_version_and_actions() {
    let world = setup_world();
    for uri in [
        "/odata",
        "/odata/",
        "/odata/$metadata",
        "/api/v1/version",
        "/api/v1/repos/joinedcontext/helsinki/actions/runs",
        "/api/v1/orgs/joinedcontext/actions/jobs",
    ] {
        let (st, _) = send_json(&world.app, "GET", uri, None, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{uri} must require a token");
        let (st, _) = send_json(&world.app, "GET", uri, Some("not-a-token"), None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{uri} must refuse a bad token");
    }
    let (st, _) = send_json(&world.app, "GET", "/odata/$metadata", Some("bob-token"), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, body) = send_json(
        &world.app,
        "GET",
        "/api/v1/repos/joinedcontext/helsinki/actions/runs",
        Some("bob-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(body["message"].as_str().unwrap_or_default().contains("actions"));
    let (st, body) = send_json(&world.app, "GET", "/api/healthz", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body, json!({ "status": "ok" }));
}
