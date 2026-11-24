#![forbid(unsafe_code)]
//! The pages' answers under load and on the records the journeys in `pages.rs` do not make: a
//! git memory budget another job holds is a 503 with `Retry-After`, never a missing page; a file
//! past the spool budget names that budget instead of a 404; a pull request stored without a
//! merge base diffs from its base.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http_body_util::BodyExt;
use shako_api::ShakoState;
use shako_api::service::{
    CommitRequest, FileChange, NewPullRequest, NewRepository, commit_files, create_branch,
    create_repository, open_pull_request,
};
use shako_api::work::{GitWork, MEMORY_BYTES, SPOOL_BYTES};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, Principal, RequestCtx, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_git::FileOpKind;
use shako_store_memory::MemoryStore;
use shako_web::web_router;
use std::sync::Arc;
use tower::ServiceExt;

const ADMIN: &str = "portal-token-0123456789";
const MIB: usize = 1024 * 1024;

fn org() -> OrgId {
    OrgId::from_u128(10)
}

fn put(path: &str, bytes: &[u8]) -> FileChange {
    FileChange {
        operation: FileOpKind::Upload,
        path: path.into(),
        content: Some(STANDARD.encode(bytes)),
        sha: None,
    }
}

fn commit(message: &str, branch: Option<&str>, files: Vec<FileChange>) -> CommitRequest {
    CommitRequest {
        branch: branch.map(Into::into),
        message: message.into(),
        author: None,
        committer: None,
        files,
    }
}

/// A shako with `joinedcontext/helsinki`: a README, a small and a 2 MiB file on main, and a pull
/// request from `feature`.
async fn shako(tmp: &tempfile::TempDir) -> ShakoState {
    let admin = Subject::authenticated("jc-portal", "jc-portal", vec![org()]);
    let state = ShakoState::new(
        InstanceId::from_u128(1),
        tmp.path().to_path_buf(),
        Arc::new(MemoryStore::new()),
        Arc::new(LocalAuthorizer::new(vec![RoleBinding {
            org_id: org(),
            principal: Principal::User("jc-portal".into()),
            actions: vec![Action::Admin, Action::Write, Action::Read],
            project_id: None,
            repo: None,
        }])),
        TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
            .with_static_tokens(vec![StaticTokenBinding::new(ADMIN, admin.clone())]),
        OutboundGuard::default(),
    );
    let ctx = RequestCtx::new(InstanceId::from_u128(1), org(), admin);
    let repo = NewRepository {
        owner: "joinedcontext".into(),
        name: "helsinki".into(),
        description: "Helsinki".into(),
        private: true,
        auto_init: true,
        default_branch: "main".into(),
    };
    create_repository(&state, &ctx, repo).await.expect("repo");
    let files = vec![
        put("notes.txt", b"hello\n"),
        put("big.txt", &vec![b'b'; 2 * MIB]),
    ];
    commit_files(
        &state,
        &ctx,
        "joinedcontext",
        "helsinki",
        commit("add", None, files),
    )
    .await
    .expect("commit");
    create_branch(&state, &ctx, "joinedcontext", "helsinki", "feature", None)
        .await
        .expect("branch");
    let change = vec![put("feature.txt", b"f\n")];
    let work = commit("feature work", Some("feature"), change);
    commit_files(&state, &ctx, "joinedcontext", "helsinki", work)
        .await
        .expect("feature commit");
    let pr = NewPullRequest {
        head: "feature".into(),
        base: "main".into(),
        title: "Feature".into(),
        body: String::new(),
    };
    open_pull_request(&state, &ctx, "joinedcontext", "helsinki", pr)
        .await
        .expect("pull request");
    state
}

async fn get(app: &Router, path: &str) -> (StatusCode, Option<String>, String) {
    let req = Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, format!("token {ADMIN}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let retry = resp
        .headers()
        .get(header::RETRY_AFTER)
        .map(|v| v.to_str().unwrap().to_owned());
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, retry, String::from_utf8_lossy(&body).into_owned())
}

/// Asks `path` while another Organization's git job holds the whole memory budget.
async fn while_memory_is_held(path: &str) -> (StatusCode, Option<String>, String) {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = shako(&tmp).await;
    state.git_work = GitWork::new(tmp.path().join("spool"), SPOOL_BYTES, 1);
    let (held_tx, held_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let holder = tokio::spawn({
        let state = state.clone();
        async move {
            state
                .git_work
                .run(OrgId::from_u128(99), move |_admit| {
                    let _ = held_tx.send(());
                    let _ = done_rx.recv();
                    Ok(())
                })
                .await
        }
    });
    held_rx.await.expect("the budget is held");
    let answer = get(&web_router().with_state(state), path).await;
    let _ = done_tx.send(());
    holder.await.expect("holder").expect("the holder's job");
    answer
}

fn assert_overloaded((status, retry, body): (StatusCode, Option<String>, String), page: &str) {
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{page}: {body}");
    assert_eq!(retry.as_deref(), Some("5"), "{page}");
    assert!(body.contains("git_memory_bytes"), "{page}: {body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_home_page_answers_busy_while_the_memory_budget_is_held() {
    let answer = while_memory_is_held("/joinedcontext/helsinki").await;
    assert_overloaded(answer, "home");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_file_page_answers_busy_while_the_memory_budget_is_held() {
    let answer = while_memory_is_held("/joinedcontext/helsinki/blob/main/notes.txt").await;
    assert_overloaded(answer, "file");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_raw_file_answers_busy_while_the_memory_budget_is_held() {
    let answer = while_memory_is_held("/joinedcontext/helsinki/raw/main/notes.txt").await;
    assert_overloaded(answer, "raw");
}

#[tokio::test]
async fn a_raw_file_past_the_spool_budget_names_the_budget_and_a_small_one_is_served() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = shako(&tmp).await;
    // One MiB of spool: `notes.txt` fits, the 2 MiB `big.txt` does not.
    state.git_work = GitWork::new(tmp.path().join("spool"), MIB as u64, MEMORY_BYTES);
    let app = web_router().with_state(state);

    let (status, _, body) = get(&app, "/joinedcontext/helsinki/raw/main/big.txt").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("spool_bytes"), "{body}");
    assert!(!body.contains("404"), "{body}");

    let (status, _, body) = get(&app, "/joinedcontext/helsinki/raw/main/notes.txt").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "hello\n");

    // An absent file under the same budget is still absent.
    let (status, _, _) = get(&app, "/joinedcontext/helsinki/raw/main/absent.txt").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_pull_request_without_a_merge_base_diffs_from_its_base() {
    let tmp = tempfile::tempdir().unwrap();
    let state = shako(&tmp).await;
    let store = state.store();
    let repo = store
        .get_repo_by_name(InstanceId::from_u128(1), org(), "joinedcontext", "helsinki")
        .await
        .expect("read")
        .expect("repo");
    let mut pr = store
        .get_pull_request(InstanceId::from_u128(1), org(), repo.id, 1)
        .await
        .expect("read")
        .expect("pull request");
    assert!(!pr.merge_base.is_empty(), "opened with a merge base");
    pr.merge_base = String::new();
    store.update_pull_request(pr).await.expect("update");

    let app = web_router().with_state(state);
    let (status, _, body) = get(&app, "/joinedcontext/helsinki/pulls/1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Changed Files (1)"), "{body}");
    assert!(body.contains("feature.txt"), "{body}");
}
