#![forbid(unsafe_code)]
//! Every page of shako-web as a caller with a bearer reads it: the repository list, home, file,
//! raw file, branches, tags, commits, one commit, pull requests and one pull request, each with
//! its 404 for what does not exist, plus the Gitea page-path redirects under a mount prefix.

use axum::Router;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http_body_util::BodyExt;
use jc_router::Mount;
use shako_api::ShakoState;
use shako_api::service::{
    CommitRequest, FileChange, NewPullRequest, NewRepository, NewReview, commit_files,
    create_branch, create_repository, create_review, open_pull_request,
};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, Principal, RequestCtx, ReviewVerdict, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_git::FileOpKind;
use shako_store_memory::MemoryStore;
use shako_web::web_router;
use std::sync::Arc;
use tower::ServiceExt;

const ADMIN: &str = "portal-token-0123456789";
const READER: &str = "reader-token-0123456789";
const README: &str = "# Helsinki\n\n<b>raw</b>\n\n![i](data:x) [a](/local) [b](mailto:a@b.example) \
[c](ftp://x) [d](#top) [e](//evil) [f](vbscript:x) [g](http://city.example)\n";

struct Shako {
    app: Router,
    first: String,
    second: String,
}

fn put(path: &str, bytes: &[u8]) -> FileChange {
    FileChange {
        operation: FileOpKind::Upload,
        path: path.into(),
        content: Some(STANDARD.encode(bytes)),
        sha: None,
    }
}

/// A shako with `joinedcontext/helsinki` (two commits on main, a branch `feature` with a pull
/// request and a review, a tag `v1`) and `joinedcontext/hidden`, which the reader may not read.
async fn shako(tmp: &tempfile::TempDir) -> Shako {
    let org = OrgId::from_u128(10);
    let admin = Subject::authenticated("jc-portal", "jc-portal", vec![org]);
    let reader = Subject::authenticated("reader", "reader", vec![org]);
    let state = ShakoState::new(
        InstanceId::from_u128(1),
        tmp.path().to_path_buf(),
        Arc::new(MemoryStore::new()),
        Arc::new(LocalAuthorizer::new(vec![
            RoleBinding {
                org_id: org,
                principal: Principal::User("jc-portal".into()),
                actions: vec![Action::Admin, Action::Write, Action::Read],
                project_id: None,
                repo: None,
            },
            RoleBinding {
                org_id: org,
                principal: Principal::User("reader".into()),
                actions: vec![Action::Read],
                project_id: None,
                repo: Some("helsinki".into()),
            },
        ])),
        TokenVerifier::new(ShakoSigningKey::generate().expect("key")).with_static_tokens(vec![
            StaticTokenBinding::new(ADMIN, admin.clone()),
            StaticTokenBinding::new(READER, reader),
        ]),
        shako_core::OutboundGuard::default(),
    );
    let ctx = RequestCtx::new(InstanceId::from_u128(1), org, admin);
    for name in ["helsinki", "hidden"] {
        let repo = NewRepository {
            owner: "joinedcontext".into(),
            name: name.into(),
            description: format!("{name} description"),
            private: true,
            // `hidden` stays empty: no branch, no README.
            auto_init: name == "helsinki",
            default_branch: "main".into(),
        };
        create_repository(&state, &ctx, repo).await.expect("repo");
    }
    let commit = |message: &str, branch: Option<&str>, files: Vec<FileChange>| CommitRequest {
        branch: branch.map(Into::into),
        message: message.into(),
        author: None,
        committer: None,
        files,
    };
    let first = commit_files(
        &state,
        &ctx,
        "joinedcontext",
        "helsinki",
        commit(
            "add files",
            None,
            vec![
                put("README.md", README.as_bytes()),
                put("docs/logo.svg", b"<svg/>"),
                put("page.HTML", b"<p>x</p>"),
                put("notes.txt", b"hello\n"),
                put("bin.dat", &[0xff, 0x00]),
                put("big.txt", &vec![b'a'; 1024 * 1024 + 2]),
            ],
        ),
    )
    .await
    .expect("first commit");
    let second = commit_files(
        &state,
        &ctx,
        "joinedcontext",
        "helsinki",
        commit("add more", None, vec![put("more.txt", b"more\n")]),
    )
    .await
    .expect("second commit");
    create_branch(&state, &ctx, "joinedcontext", "helsinki", "feature", None)
        .await
        .expect("branch");
    commit_files(
        &state,
        &ctx,
        "joinedcontext",
        "helsinki",
        commit(
            "feature work",
            Some("feature"),
            vec![put("feature.txt", b"f\n")],
        ),
    )
    .await
    .expect("feature commit");
    open_pull_request(
        &state,
        &ctx,
        "joinedcontext",
        "helsinki",
        NewPullRequest {
            head: "feature".into(),
            base: "main".into(),
            title: "Feature title".into(),
            body: "Body <script>alert(1)</script> [x](javascript:alert(2))".into(),
        },
    )
    .await
    .expect("pull request");
    create_review(
        &state,
        &ctx,
        "joinedcontext",
        "helsinki",
        1,
        NewReview {
            verdict: ReviewVerdict::Comment,
            body: "looks fine".into(),
            commit_id: None,
        },
    )
    .await
    .expect("review");
    let (_, disk) = shako_api::resolve_and_authorize_repo(
        &state,
        &ctx,
        "joinedcontext",
        "helsinki",
        Action::Read,
    )
    .await
    .expect("disk path");
    let tagged = std::process::Command::new("git")
        .arg("-C")
        .arg(&disk)
        .args(["tag", "v1", &first])
        .status()
        .expect("git runs");
    assert!(tagged.success());
    Shako {
        app: web_router().with_state(state),
        first,
        second,
    }
}

/// Status, headers of interest and body of a GET with `token`.
async fn get(app: &Router, path: &str, token: &str) -> (StatusCode, [String; 3], String) {
    let req = Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, format!("token {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let h = |name: header::HeaderName| {
        resp.headers()
            .get(name)
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default()
    };
    let heads = [
        h(header::CONTENT_TYPE),
        h(header::CONTENT_DISPOSITION),
        h(header::X_CONTENT_TYPE_OPTIONS),
    ];
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, heads, String::from_utf8_lossy(&body).into_owned())
}

async fn page(app: &Router, path: &str) -> String {
    let (status, _, body) = get(app, path, ADMIN).await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    body
}

#[tokio::test]
async fn the_repository_list_shows_only_what_the_caller_may_read() {
    let tmp = tempfile::tempdir().unwrap();
    let s = shako(&tmp).await;
    let all = page(&s.app, "/").await;
    assert!(all.contains("joinedcontext/helsinki"), "{all}");
    assert!(all.contains("joinedcontext/hidden"), "{all}");
    let (status, _, mine) = get(&s.app, "/", READER).await;
    assert_eq!(status, StatusCode::OK);
    assert!(mine.contains("joinedcontext/helsinki"), "{mine}");
    assert!(!mine.contains("hidden"), "{mine}");
    let (status, _, _) = get(&s.app, "/", "not-a-token").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let mounted = s.app.clone().layer(Extension(Mount {
        path_prefix: "/git".into(),
        ..Mount::default()
    }));
    assert!(
        page(&mounted, "/")
            .await
            .contains("href=\"/git/joinedcontext/helsinki\"")
    );
}

#[tokio::test]
async fn the_home_page_lists_files_and_renders_the_readme_safely() {
    let tmp = tempfile::tempdir().unwrap();
    let s = shako(&tmp).await;
    let home = page(&s.app, "/joinedcontext/helsinki").await;
    assert!(home.contains("helsinki description"), "{home}");
    assert!(
        home.contains("/joinedcontext/helsinki/blob/main/notes.txt"),
        "{home}"
    );
    assert!(home.contains("<h2>Helsinki</h2>"), "{home}");
    assert!(home.contains("&lt;b&gt;raw&lt;/b&gt;"), "{home}");
    for safe in [
        "/local",
        "mailto:a@b.example",
        "#top",
        "http://city.example",
    ] {
        assert!(home.contains(&format!("href=\"{safe}\"")), "{safe}: {home}");
    }
    for unsafe_url in ["data:x", "ftp://x", "//evil", "vbscript:x"] {
        assert!(!home.contains(unsafe_url), "{unsafe_url}: {home}");
    }
    // An empty repository shows no file and no README section.
    let hidden = page(&s.app, "/joinedcontext/hidden").await;
    assert!(!hidden.contains("/blob/"), "{hidden}");
    assert!(!hidden.contains("README"), "{hidden}");
    let (status, _, _) = get(&s.app, "/joinedcontext/hidden", READER).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no right reads as missing");
}

#[tokio::test]
async fn the_file_page_shows_a_file_and_says_when_it_shows_only_the_first_mib() {
    let tmp = tempfile::tempdir().unwrap();
    let s = shako(&tmp).await;
    let notes = page(&s.app, "/joinedcontext/helsinki/blob/main/notes.txt").await;
    assert!(notes.contains("hello"), "{notes}");
    assert!(!notes.contains("first MiB"), "{notes}");
    assert!(notes.contains("/joinedcontext/helsinki/raw/main/notes.txt"));
    let big = page(&s.app, "/joinedcontext/helsinki/blob/main/big.txt").await;
    assert!(big.contains("This page shows the first MiB of the file."));
    for missing in [
        "/joinedcontext/helsinki/blob/main/none.txt",
        "/joinedcontext/helsinki/blob/nobranch/notes.txt",
        "/joinedcontext/missing/blob/main/notes.txt",
    ] {
        assert_eq!(
            get(&s.app, missing, ADMIN).await.0,
            StatusCode::NOT_FOUND,
            "{missing}"
        );
    }
}

#[tokio::test]
async fn a_raw_file_is_never_rendered_by_the_browser() {
    let tmp = tempfile::tempdir().unwrap();
    let s = shako(&tmp).await;
    for (path, content_type, disposition) in [
        (
            "notes.txt",
            "text/plain; charset=utf-8",
            "attachment; filename=\"notes.txt\"",
        ),
        (
            "docs/logo.svg",
            "application/octet-stream",
            "attachment; filename=\"logo.svg\"",
        ),
        (
            "page.HTML",
            "application/octet-stream",
            "attachment; filename=\"page.HTML\"",
        ),
        (
            "bin.dat",
            "application/octet-stream",
            "attachment; filename=\"bin.dat\"",
        ),
    ] {
        let uri = format!("/joinedcontext/helsinki/raw/main/{path}");
        let (status, heads, _) = get(&s.app, &uri, ADMIN).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(
            heads,
            [content_type, disposition, "nosniff"].map(String::from),
            "{path}"
        );
    }
    let (_, _, body) = get(&s.app, "/joinedcontext/helsinki/raw/main/notes.txt", ADMIN).await;
    assert_eq!(body, "hello\n");
    for missing in [
        "/joinedcontext/helsinki/raw/main/none.txt",
        "/joinedcontext/missing/raw/main/notes.txt",
    ] {
        assert_eq!(
            get(&s.app, missing, ADMIN).await.0,
            StatusCode::NOT_FOUND,
            "{missing}"
        );
    }
    let (status, _, _) = get(&s.app, "/joinedcontext/helsinki/raw/main/notes.txt", "bad").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn branches_tags_and_commits_list_their_refs() {
    let tmp = tempfile::tempdir().unwrap();
    let s = shako(&tmp).await;
    let branches = page(&s.app, "/joinedcontext/helsinki/branches").await;
    assert!(branches.contains("<strong>main</strong>"), "{branches}");
    assert!(branches.contains("<strong>feature</strong>"), "{branches}");
    let tags = page(&s.app, "/joinedcontext/helsinki/tags").await;
    assert!(
        tags.contains(&format!("<strong>v1</strong> <code>{}</code>", s.first)),
        "{tags}"
    );
    let commits = page(&s.app, "/joinedcontext/helsinki/commits/main").await;
    assert!(commits.contains("add files"), "{commits}");
    assert!(commits.contains(&format!("/joinedcontext/helsinki/commit/{}", s.second)));
    assert!(!commits.contains("feature work"), "{commits}");
    // An unknown ref lists no commit.
    let none = page(&s.app, "/joinedcontext/helsinki/commits/nobranch").await;
    assert!(!none.contains("/commit/"), "{none}");
    for path in [
        "/joinedcontext/missing/branches",
        "/joinedcontext/missing/tags",
        "/joinedcontext/missing/commits/main",
    ] {
        assert_eq!(
            get(&s.app, path, ADMIN).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
        assert_eq!(
            get(&s.app, path, "bad").await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_commit_page_lists_the_files_it_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let s = shako(&tmp).await;
    let second = page(
        &s.app,
        &format!("/joinedcontext/helsinki/commit/{}", s.second),
    )
    .await;
    assert!(second.contains("add more"), "{second}");
    assert!(second.contains("more.txt"), "{second}");
    assert!(!second.contains("notes.txt"), "{second}");
    let first = page(
        &s.app,
        &format!("/joinedcontext/helsinki/commit/{}", s.first),
    )
    .await;
    assert!(first.contains("notes.txt"), "{first}");
    for path in [
        "/joinedcontext/helsinki/commit/0000000000000000000000000000000000000000".to_string(),
        format!("/joinedcontext/missing/commit/{}", s.first),
    ] {
        assert_eq!(
            get(&s.app, &path, ADMIN).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    let path = format!("/joinedcontext/helsinki/commit/{}", s.first);
    assert_eq!(get(&s.app, &path, "bad").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_pull_request_pages_show_the_request_its_files_and_reviews() {
    let tmp = tempfile::tempdir().unwrap();
    let s = shako(&tmp).await;
    let list = page(&s.app, "/joinedcontext/helsinki/pulls").await;
    assert!(list.contains("#1 Feature title"), "{list}");
    assert!(list.contains("feature &rarr; main"), "{list}");
    let empty = page(&s.app, "/joinedcontext/hidden/pulls").await;
    assert!(!empty.contains("/pulls/"), "{empty}");
    let detail = page(&s.app, "/joinedcontext/helsinki/pulls/1").await;
    assert!(detail.contains("feature.txt"), "{detail}");
    assert!(detail.contains("Changed Files (1)"), "{detail}");
    assert!(detail.contains("Reviews (1)"), "{detail}");
    assert!(detail.contains("[COMMENT]: looks fine"), "{detail}");
    assert!(detail.contains("&lt;script&gt;"), "{detail}");
    assert!(!detail.contains("javascript:"), "{detail}");
    for path in [
        "/joinedcontext/helsinki/pulls/2",
        "/joinedcontext/missing/pulls/1",
        "/joinedcontext/missing/pulls",
    ] {
        assert_eq!(
            get(&s.app, path, ADMIN).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    for path in [
        "/joinedcontext/helsinki/pulls/1",
        "/joinedcontext/helsinki/pulls",
    ] {
        assert_eq!(
            get(&s.app, path, "bad").await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
}

#[tokio::test]
async fn actions_pages_are_not_served_yet() {
    let tmp = tempfile::tempdir().unwrap();
    let s = shako(&tmp).await;
    for path in [
        "/joinedcontext/helsinki/actions",
        "/joinedcontext/helsinki/actions/runs/1",
    ] {
        let (status, _, body) = get(&s.app, path, ADMIN).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::NOT_FOUND, "<h1>404 Not Found</h1>")
        );
    }
}

#[tokio::test]
async fn gitea_page_paths_redirect_under_the_mount_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let s = shako(&tmp).await;
    let mounted = s.app.clone().layer(Extension(Mount {
        path_prefix: "/git".into(),
        ..Mount::default()
    }));
    for (app, prefix) in [(&s.app, ""), (&mounted, "/git")] {
        for (path, target) in [
            (
                "/joinedcontext/helsinki/src/branch/main/docs/logo.svg",
                format!("{prefix}/joinedcontext/helsinki/blob/main/docs/logo.svg"),
            ),
            (
                "/joinedcontext/-/packages/generic/tool/1.0",
                format!("{prefix}/joinedcontext/packages/generic/tool/1.0"),
            ),
            (
                "/user/oauth2/keycloak?redirect_to=/joinedcontext/helsinki",
                "/joinedcontext/helsinki".into(),
            ),
            (
                "/user/oauth2/keycloak",
                if prefix.is_empty() {
                    "/".into()
                } else {
                    prefix.into()
                },
            ),
            (
                "/user/oauth2/keycloak?redirect_to=/a%5Cb",
                if prefix.is_empty() {
                    "/".into()
                } else {
                    prefix.into()
                },
            ),
        ] {
            let req = Request::builder().uri(path).body(Body::empty()).unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "{path}");
            assert_eq!(
                resp.headers()[header::LOCATION].to_str().unwrap(),
                target,
                "{path} under '{prefix}'"
            );
        }
    }
}
