//! T-0190: shako OData client integration tests with wiremock.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::forge::{
    Author, FileDelete, FileWrite, GitError, MergeStyle, RepoFile, ReviewEvent, ShakoClient,
};
use serde_json::json;
use wiremock::matchers::{body_json, body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn default_branch_and_branch_head() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "DefaultBranch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Branches('main')",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Name": "main",
            "HeadSha": "c0ffee1234567890abcdef"
        })))
        .mount(&server)
        .await;

    let branch = client.default_branch().await.unwrap();
    assert_eq!(branch, "main");

    let head = client.branch_head("main").await.unwrap();
    assert_eq!(head, "c0ffee1234567890abcdef");
}

#[tokio::test]
async fn create_branch_success_and_conflict() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.CreateBranch",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .and(body_json(json!({
            "NewBranchName": "feature-1",
            "OldBranchName": "main"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "Name": "feature-1"
        })))
        .mount(&server)
        .await;

    client.create_branch("feature-1", "main").await.unwrap();

    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.CreateBranch",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .and(body_json(json!({
            "NewBranchName": "already-exists",
            "OldBranchName": "main"
        })))
        .respond_with(ResponseTemplate::new(409).set_body_string("branch already exists"))
        .mount(&server)
        .await;

    let err = client
        .create_branch("already-exists", "main")
        .await
        .unwrap_err();
    match err {
        GitError::Conflict(msg) => assert!(msg.contains("branch already exists")),
        other => panic!("expected GitError::Conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn get_file_decodes_base64_and_handles_404() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    let raw_text = "apiVersion: joinedcontext.com/v1alpha1\nkind: Dashboard\n";
    // A forge may wrap the base64 payload, so the fixture is built with the line breaks in it
    // rather than pasted — a hand-typed constant is one typo away from invalid padding.
    let encoded = base64::engine::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        raw_text.as_bytes(),
    );
    let b64_with_newlines = format!("{}\n{}\n", &encoded[..16], &encoded[16..]);

    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.File(Path=@p,Ref=@r)",
        ))
        .and(query_param("@p", "'dashboards/dash.yaml'"))
        .and(header("authorization", "Bearer secret-token"))
        .and(query_param("@r", "'main'"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "sha-file-123",
            "Content": b64_with_newlines.clone()
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.File(Path=@p,Ref=@r)",
        ))
        .and(query_param("@p", "'missing.yaml'"))
        .and(header("authorization", "Bearer secret-token"))
        .and(query_param("@r", "'main'"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "message": "not found"
        })))
        .mount(&server)
        .await;

    let file = client
        .get_file("dashboards/dash.yaml", "main")
        .await
        .unwrap();
    assert_eq!(
        file,
        Some(RepoFile {
            sha: "sha-file-123".to_string(),
            content: raw_text.to_string(),
        })
    );

    let missing = client.get_file("missing.yaml", "main").await.unwrap();
    assert_eq!(missing, None);
}

#[tokio::test]
async fn put_file_creates_and_replaces() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    // Create (sha is None -> omitted from JSON)
    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.CommitFiles",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .and(body_json(json!({
            "Branch": "feature-1",
            "Message": "create dashboard",
            "Author": { "Name": "Alice Developer", "Email": "alice@example.com" },
            "Committer": { "Name": "Alice Developer", "Email": "alice@example.com" },
            "Files": [{
                "operation": "upload",
                "path": "dashboards/dash.yaml",
                "content": "dGl0bGU6IGhlbGxvIHdvcmxkCg=="
            }]
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "Sha": "commit-sha-created"
        })))
        .mount(&server)
        .await;

    let write_create = FileWrite {
        path: "dashboards/dash.yaml",
        branch: "feature-1",
        message: "create dashboard",
        content: "title: hello world\n",
        sha: None,
        author: Author {
            name: "Alice Developer",
            email: "alice@example.com",
        },
    };
    let sha_created = client.put_file(&write_create).await.unwrap();
    assert_eq!(sha_created, "commit-sha-created");

    // Replace (sha is Some -> present in JSON)
    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.CommitFiles",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .and(body_json(json!({
            "Branch": "feature-1",
            "Message": "update dashboard",
            "Author": { "Name": "Alice Developer", "Email": "alice@example.com" },
            "Committer": { "Name": "Alice Developer", "Email": "alice@example.com" },
            "Files": [{
                "operation": "update",
                "path": "dashboards/dash.yaml",
                "content": "dGl0bGU6IGhlbGxvIHdvcmxkIHYyCg==",
                "sha": "old-sha-111"
            }]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "commit-sha-updated"
        })))
        .mount(&server)
        .await;

    let write_replace = FileWrite {
        path: "dashboards/dash.yaml",
        branch: "feature-1",
        message: "update dashboard",
        content: "title: hello world v2\n",
        sha: Some("old-sha-111"),
        author: Author {
            name: "Alice Developer",
            email: "alice@example.com",
        },
    };
    let sha_updated = client.put_file(&write_replace).await.unwrap();
    assert_eq!(sha_updated, "commit-sha-updated");
}

#[tokio::test]
async fn delete_file_sends_sha() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.CommitFiles",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .and(body_json(json!({
            "Branch": "feature-1",
            "Message": "remove dashboard",
            "Author": { "Name": "Bob Admin", "Email": "bob@example.com" },
            "Committer": { "Name": "Bob Admin", "Email": "bob@example.com" },
            "Files": [{
                "operation": "delete",
                "path": "dashboards/dash.yaml",
                "sha": "delete-sha-222"
            }]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "commit-sha-deleted"
        })))
        .mount(&server)
        .await;

    let req = FileDelete {
        path: "dashboards/dash.yaml",
        branch: "feature-1",
        message: "remove dashboard",
        sha: "delete-sha-222",
        author: Author {
            name: "Bob Admin",
            email: "bob@example.com",
        },
    };
    let sha = client.delete_file(&req).await.unwrap();
    assert_eq!(sha, "commit-sha-deleted");
}

#[tokio::test]
async fn create_and_get_pull_request() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.OpenPullRequest",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .and(body_json(json!({
            "Head": "feature-1",
            "Base": "main",
            "Title": "Add feature",
            "Body": "PR description"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "Number": 17,
            "HtmlUrl": "https://shako.example.com/test-owner/test-repo/pulls/17",
            "State": "open",
            "Mergeable": true,
            "Merged": false
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/PullRequests(17)",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Number": 17,
            "HtmlUrl": "https://shako.example.com/test-owner/test-repo/pulls/17",
            "State": "closed",
            "Mergeable": null,
            "Merged": true
        })))
        .mount(&server)
        .await;

    let pr = client
        .create_pull_request("feature-1", "main", "Add feature", "PR description")
        .await
        .unwrap();
    assert_eq!(pr.number, 17);
    assert_eq!(
        pr.url,
        "https://shako.example.com/test-owner/test-repo/pulls/17"
    );
    assert_eq!(pr.state, "open");
    assert_eq!(pr.mergeable, Some(true));
    assert!(!pr.merged);

    let fetched = client.pull_request(17).await.unwrap();
    assert_eq!(fetched.number, 17);
    assert_eq!(fetched.state, "closed");
    assert_eq!(fetched.mergeable, None);
    assert!(fetched.merged);

    // With a public forge the link a person opens is the public one, never the forge's internal address
    // (AP-71), and it goes through the forge's own sign-in so a Portal session that has no
    // forge session meets the Keycloak button instead of a 404 (PF-81, T-0702).
    let mut public = client.clone();
    public.public_base = "https://city.example/git".parse().unwrap();
    assert_eq!(
        public.pull_request(17).await.unwrap().url,
        "https://city.example/git/user/oauth2/keycloak?redirect_to=%2Fgit%2Ftest-owner%2Ftest-repo%2Fpulls%2F17"
    );
}

#[tokio::test]
async fn review_and_merge() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("POST"))
        .and(path("/odata/Repositories(Owner='test-owner',Name='test-repo')/PullRequests(17)/Shako.Review"))
        .and(header("authorization", "Bearer secret-token"))
        .and(body_json(json!({
            "Event": "APPROVED",
            "Body": "Approved by security steward"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    client
        .review(17, ReviewEvent::Approve, "Approved by security steward")
        .await
        .unwrap();

    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/PullRequests(17)/Shako.Merge",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .and(body_json(json!({
            "Style": "merge",
            "Message": "Merge PR #17",
            "HeadCommitId": "reviewed-commit"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    // The commit the caller reviewed travels with the merge, so the forge refuses a branch that
    // moved past it (T-1683).
    client
        .merge(
            17,
            MergeStyle::Merge,
            "Merge PR #17",
            Some("reviewed-commit"),
        )
        .await
        .unwrap();
}

/// T-1683: a merge with nothing to pin it to — a proposal this Portal wrote and merges in the
/// same call — sends no `HeadCommitId` at all, rather than an empty one the forge would refuse.
#[tokio::test]
async fn merge_without_a_reviewed_commit_sends_no_head_commit_id() {
    let server = MockServer::start().await;
    let client = ShakoClient::new(
        server.uri().parse().unwrap(),
        "test-owner",
        "test-repo",
        "secret-token",
    )
    .unwrap();
    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/PullRequests(18)/Shako.Merge",
        ))
        .and(body_json(json!({
            "Style": "squash",
            "Message": "Merge PR #18"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    for pinned in [None, Some("")] {
        client
            .merge(18, MergeStyle::Squash, "Merge PR #18", pinned)
            .await
            .unwrap();
    }
}

#[test]
fn shako_client_redacts_token_in_debug() {
    let url = "https://shako.example.com".parse().unwrap();
    let client = ShakoClient::new(url, "owner", "repo", "my-secret-token").unwrap();
    let debug = format!("{client:?}");
    assert!(!debug.contains("my-secret-token"));
    assert!(debug.contains("[redacted]"));
}

#[test]
fn from_env_fail_closed_behavior() {
    let empty = ShakoClient::from_env(|_| None).unwrap();
    assert!(empty.is_none());

    let err1 = ShakoClient::from_env(|k| match k {
        "JC_SHAKO_URL" => Some("https://shako.example.com".to_string()),
        _ => None,
    })
    .unwrap_err();
    match err1 {
        GitError::Config(msg) => assert!(msg.contains("must be set together")),
        other => panic!("expected Config error, got {other:?}"),
    }

    let err2 = ShakoClient::from_env(|k| match k {
        "JC_SHAKO_URL" => Some("https://shako.example.com".to_string()),
        "JC_SHAKO_OWNER" => Some("owner".to_string()),
        "JC_SHAKO_REPO" => Some("repo".to_string()),
        _ => None,
    })
    .unwrap_err();
    assert!(matches!(err2, GitError::Config(_)));

    let err_url = ShakoClient::from_env(|k| match k {
        "JC_SHAKO_URL" => Some("not-a-valid-url".to_string()),
        "JC_SHAKO_OWNER" => Some("owner".to_string()),
        "JC_SHAKO_REPO" => Some("repo".to_string()),
        "JC_SHAKO_TOKEN" => Some("token".to_string()),
        _ => None,
    })
    .unwrap_err();
    assert!(matches!(err_url, GitError::Config(_)));

    let complete = ShakoClient::from_env(|k| match k {
        "JC_SHAKO_URL" => Some("https://shako.example.com".to_string()),
        "JC_SHAKO_OWNER" => Some("my-owner".to_string()),
        "JC_SHAKO_REPO" => Some("my-repo".to_string()),
        "JC_SHAKO_TOKEN" => Some("my-token".to_string()),
        _ => None,
    })
    .unwrap()
    .unwrap();
    assert_eq!(complete.owner, "my-owner");
    assert_eq!(complete.repo, "my-repo");
}

#[tokio::test]
async fn list_tree_filters_blobs_and_detects_truncated() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("GET"))
        .and(path("/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)")).and(query_param("@r", "\'main\'"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "tree-sha-1",
            "Truncated": false,
            "Tree": [
                {
                    "Path": "projects/ovzdusie/spaces/ovzdusie/space.yaml",
                    "Type": "blob"
                },
                {
                    "Path": "projects/ovzdusie",
                    "Type": "tree"
                }
            ]
        })))
        .mount(&server)
        .await;

    let paths = client.list_tree("main").await.unwrap();
    assert_eq!(paths, vec!["projects/ovzdusie/spaces/ovzdusie/space.yaml"]);

    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)",
        )).and(query_param("@r", "\'truncated-ref\'"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "tree-sha-2",
            "Truncated": true,
            "Tree": []
        })))
        .mount(&server)
        .await;

    let err = client.list_tree("truncated-ref").await.unwrap_err();
    match err {
        GitError::Api { message, .. } => assert!(message.contains("truncated")),
        other => panic!("expected GitError::Api, got {other:?}"),
    }
}

/// T-5047: the forge hands a recursive tree out 1000 entries a page; a project of 1321 files
/// read only page 1 and refused to stage. Every page is read now, and a tree whose sha changes
/// between pages (a push in between) is refused rather than mixed.
#[tokio::test]
async fn list_tree_reads_every_page_of_a_large_tree() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    let entries = |from: usize, to: usize| -> Vec<serde_json::Value> {
        (from..to)
            .map(|i| json!({ "Path": format!("seed/f{i}.yaml"), "Type": "blob", "Sha": format!("b{i}") }))
            .collect()
    };
    let page = |ref_: &'static str,
                n: &'static str,
                sha: &str,
                tree: Vec<serde_json::Value>,
                truncated: bool| {
        Mock::given(method("GET"))
            .and(path("/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)")).and(query_param("@r", format!("'{ref_}'")))
            .and(query_param("@s", n))
            .and(query_param("@t", "1000"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "Sha": sha, "Truncated": truncated, "Tree": tree })),
            )
    };
    page("big", "0", "t1", entries(0, 1000), true)
        .mount(&server)
        .await;
    page("big", "1000", "t1", entries(1000, 1321), false)
        .mount(&server)
        .await;
    let blobs = client.list_tree_blobs("big").await.unwrap();
    assert_eq!(blobs.len(), 1321);
    assert_eq!(
        blobs[1320],
        ("seed/f1320.yaml".to_string(), "b1320".to_string())
    );

    page("moving", "0", "t1", entries(0, 1000), true)
        .mount(&server)
        .await;
    page("moving", "1000", "t2", entries(1000, 1001), false)
        .mount(&server)
        .await;
    match client.list_tree_blobs("moving").await.unwrap_err() {
        GitError::Api { message, .. } => assert!(message.contains("changed while it was read")),
        other => panic!("expected GitError::Api, got {other:?}"),
    }
}

#[test]
fn git_error_converts_to_api_error() {
    let not_found = GitError::NotFound;
    assert!(matches!(ApiError::from(not_found), ApiError::NotFound(_)));

    let conflict = GitError::Conflict("branch exists".into());
    assert!(matches!(ApiError::from(conflict), ApiError::Conflict(_)));

    // The forge down or refusing the Portal's own token is an outage, not the caller's fault (T-4966).
    let transport = GitError::Transport("connection reset".into());
    assert!(matches!(
        ApiError::from(transport),
        ApiError::Unavailable(_)
    ));

    let config = GitError::Config("missing var".into());
    assert!(matches!(ApiError::from(config), ApiError::Internal(_)));

    let api = GitError::Api {
        status: 500,
        message: "internal".into(),
    };
    assert!(matches!(ApiError::from(api), ApiError::Unavailable(_)));

    let refused = GitError::Api {
        status: 422,
        message: "invalid".into(),
    };
    assert!(matches!(ApiError::from(refused), ApiError::Internal(_)));
}

#[tokio::test]
async fn change_files_uploads_and_deletes_in_one_commit() {
    let server = MockServer::start().await;
    let client = ShakoClient::new(
        server.uri().parse().unwrap(),
        "test-owner",
        "test-repo",
        "secret-token",
    )
    .unwrap();
    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.CommitFiles",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .and(body_json(json!({
            "Files": [
                { "operation": "upload", "path": "apps/a/src/App.tsx", "content": "aGVsbG8=" },
                { "operation": "upload", "path": "apps/a/package.json", "content": "e30=" },
                { "operation": "delete", "path": "apps/a/src/Old.tsx", "sha": "blob-old" }
            ],
            "Message": "first run",
            "Branch": "agent/a",
            "Author": { "Name": "Alice", "Email": "alice@example.com" },
            "Committer": { "Name": "Alice", "Email": "alice@example.com" }
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "Sha": "commit-of-both"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let sha = client
        .change_files(
            "agent/a",
            "first run",
            Author {
                name: "Alice",
                email: "alice@example.com",
            },
            &[
                ("/apps/a/src/App.tsx".to_owned(), "hello".to_owned()),
                ("apps/a/package.json".to_owned(), "{}".to_owned()),
            ],
            &[("apps/a/src/Old.tsx".to_owned(), "blob-old".to_owned())],
        )
        .await
        .unwrap();
    assert_eq!(sha, "commit-of-both");
}

#[tokio::test]
async fn pull_request_files_lists_every_file_and_marks_deletions() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    // The forge answers every file of a pull request in one collection, no server paging.
    let mut rows: Vec<serde_json::Value> = (0..50)
        .map(|i| json!({ "Filename": format!("projects/p/pipelines/{i}/pipeline.yaml"), "Status": "added" }))
        .collect();
    rows.push(json!({ "Filename": "users/assignments/gone.yaml", "Status": "deleted" }));
    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/PullRequests(9)/Files",
        ))
        .and(header("authorization", "Bearer secret-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": rows })))
        .expect(1)
        .mount(&server)
        .await;

    let files = client.pull_request_files(9).await.unwrap();
    assert_eq!(files.len(), 51);
    assert!(files[..50].iter().all(|f| !f.deleted));
    assert_eq!(files[50].path, "users/assignments/gone.yaml");
    assert!(files[50].deleted);
}

/// A stale branch is dropped before a proposal starts again from main (T-0886); one already
/// gone is nothing to report.
#[tokio::test]
async fn delete_branch_drops_a_branch_and_ignores_one_already_gone() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.DeleteBranch",
        ))
        .and(body_string_contains(
            r#""Branch":"portal/delete-contextspace-x-1""#,
        ))
        .and(header("authorization", "Bearer secret-token"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.DeleteBranch",
        ))
        .and(body_string_contains(r#""Branch":"gone""#))
        .respond_with(ResponseTemplate::new(404).set_body_string("branch does not exist"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.DeleteBranch",
        ))
        .and(body_string_contains(r#""Branch":"protected""#))
        .respond_with(ResponseTemplate::new(403).set_body_string("branch is protected"))
        .mount(&server)
        .await;

    client
        .delete_branch("portal/delete-contextspace-x-1")
        .await
        .unwrap();
    client.delete_branch("gone").await.unwrap();
    let err = client.delete_branch("protected").await.unwrap_err();
    assert!(matches!(err, GitError::Api { status: 403, .. }), "{err:?}");
}

/// T-2266: a workspace branch is `workspace/{name}`. The ref travels as the `@r` parameter
/// value, so a name with a slash reads its tree directly, with no resolving round trip, and a
/// branch that does not exist yet answers "not found".
#[tokio::test]
async fn a_branch_with_a_slash_is_resolved_before_its_tree_is_read() {
    let server = MockServer::start().await;
    let client = ShakoClient::new(
        server.uri().parse().unwrap(),
        "test-owner",
        "test-repo",
        "secret-token",
    )
    .unwrap();

    Mock::given(method("GET"))
        .and(path("/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"))
        .and(query_param("@r", "'workspace/air-v2'"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Tree": [{ "Path": "projects/p/spaces/air/space.yaml", "Type": "blob", "Sha": "blob1" }],
            "Truncated": false
        })))
        .mount(&server)
        .await;

    let blobs = client.list_tree_blobs("workspace/air-v2").await.unwrap();
    assert_eq!(
        blobs,
        vec![(
            "projects/p/spaces/air/space.yaml".to_string(),
            "blob1".to_string()
        )]
    );

    // A branch that does not exist yet is still "not found", which is what a copy with no commit
    // of its own is: the caller then shows the project's own files (CC-76).
    let missing = client.list_tree_blobs("workspace/gone").await;
    assert!(matches!(missing, Err(GitError::NotFound)), "{missing:?}");

    // A plain branch name is read the same way, and no branch lookup ever happened.
    Mock::given(method("GET"))
        .and(path("/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)")).and(query_param("@r", "'main'"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "Tree": [], "Truncated": false })),
        )
        .mount(&server)
        .await;
    assert!(client.list_tree_blobs("main").await.unwrap().is_empty());
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|r| r.url.path().contains("Shako.Tree(")),
        "only tree reads: {:?}",
        requests
            .iter()
            .map(|r| r.url.path().to_owned())
            .collect::<Vec<_>>()
    );
}

/// AP-75: an application's repository is created in the organization, private and initialised on
/// `main`, when the forge does not have it.
#[tokio::test]
async fn a_missing_application_repository_is_created_private_on_main() {
    let server = MockServer::start().await;
    let config = ShakoClient::new(
        server.uri().parse().unwrap(),
        "joinedcontext",
        "configuration",
        "secret-token",
    )
    .unwrap();
    let repo = config.for_repository("helsinki_city-bikes");

    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='joinedcontext',Name='helsinki_city-bikes')",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/odata/CreateRepository"))
        .and(header("authorization", "Bearer secret-token"))
        .and(body_json(json!({
            "Owner": "joinedcontext",
            "Name": "helsinki_city-bikes",
            "Description": "the city bikes app",
            "Private": true,
            "AutoInit": true,
            "DefaultBranch": "main"
        })))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "Name": "helsinki_city-bikes" })),
        )
        .expect(1)
        .mount(&server)
        .await;

    assert!(repo.ensure_repository("the city bikes app").await.unwrap());
}

/// AP-75: an existing repository is never recreated or overwritten, and one another request
/// created a moment earlier (409) is the same answer.
#[tokio::test]
async fn an_existing_application_repository_is_left_as_it_is() {
    let server = MockServer::start().await;
    let config = ShakoClient::new(
        server.uri().parse().unwrap(),
        "joinedcontext",
        "configuration",
        "t",
    )
    .unwrap();

    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='joinedcontext',Name='helsinki_city-bikes')",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='joinedcontext',Name='helsinki_raced')",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/odata/CreateRepository"))
        .and(body_json(json!({
            "Owner": "joinedcontext",
            "Name": "helsinki_raced",
            "Description": "d",
            "Private": true,
            "AutoInit": true,
            "DefaultBranch": "main"
        })))
        .respond_with(ResponseTemplate::new(409).set_body_string("repository already exists"))
        .expect(1)
        .mount(&server)
        .await;

    assert!(!config
        .for_repository("helsinki_city-bikes")
        .ensure_repository("d")
        .await
        .unwrap());
    assert!(!config
        .for_repository("helsinki_raced")
        .ensure_repository("d")
        .await
        .unwrap());
}

/// AP-75: a forge that refuses the creation (the token lacks the organization scope) is an
/// error the run says, never a silent success.
#[tokio::test]
async fn a_refused_repository_creation_is_an_error() {
    let server = MockServer::start().await;
    let config = ShakoClient::new(
        server.uri().parse().unwrap(),
        "joinedcontext",
        "configuration",
        "t",
    )
    .unwrap();

    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='joinedcontext',Name='helsinki_city-bikes')",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/odata/CreateRepository"))
        .respond_with(
            ResponseTemplate::new(403).set_body_string("token does not have write:organization"),
        )
        .mount(&server)
        .await;

    let err = config
        .for_repository("helsinki_city-bikes")
        .ensure_repository("d")
        .await
        .unwrap_err();
    assert!(matches!(err, GitError::Api { status: 403, .. }), "{err:?}");
}

/// AP-77, AP-78: the clone address is the forge's public https one, never the cluster name.
#[test]
fn the_clone_address_is_the_public_forge() {
    let config = ShakoClient::from_env(|key| {
        match key {
            "JC_SHAKO_URL" => Some("http://shako-http.dev.svc.cluster.local:3000"),
            "JC_SHAKO_OWNER" => Some("joinedcontext"),
            "JC_SHAKO_REPO" => Some("configuration"),
            "JC_SHAKO_TOKEN" => Some("t"),
            "JC_SHAKO_PUBLIC_URL" => Some("https://2.28.67.127.sslip.io/shako/"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .unwrap()
    .unwrap();
    let repo = config.for_repository("helsinki_city-bikes");
    assert_eq!(
        repo.clone_url(),
        "https://2.28.67.127.sslip.io/shako/joinedcontext/helsinki_city-bikes.git"
    );
    assert_eq!(repo.owner, "joinedcontext");
    assert_eq!(
        config.repo, "configuration",
        "the configuration client is unchanged"
    );
}

/// T-4967: a forge error carries a message beside other members (`{"message": …, "url": …}`, or
/// OData's `{"error": {"message": …}}`); the error carries the message alone, so the activity
/// feed and a 409 answer never show the raw body.
#[tokio::test]
async fn a_forge_error_keeps_only_its_message() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();
    let body = json!({
        "message": "invalid username, password or token",
        "url": "https://forge.example/api/swagger"
    });
    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.DeleteBranch",
        ))
        .and(body_string_contains(r#""Branch":"refused""#))
        .respond_with(ResponseTemplate::new(401).set_body_json(body.clone()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.CreateBranch",
        ))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error": { "code": "Conflict", "message": "The branch already exists." }
        })))
        .mount(&server)
        .await;

    let err = client.delete_branch("refused").await.unwrap_err();
    assert_eq!(
        err,
        GitError::Api {
            status: 401,
            message: "invalid username, password or token".into()
        }
    );
    let err = client.create_branch("taken", "main").await.unwrap_err();
    assert_eq!(err, GitError::Conflict("The branch already exists.".into()));
}

/// T-5052/T-5151: a YAML file reaches the forge in the jcctl fmt layout, whatever layout the
/// caller handed in.
#[tokio::test]
async fn put_file_commits_yaml_in_the_canonical_layout() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = ShakoClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();
    let messy = "kind: Policy\napiVersion: joinedcontext.com/v1alpha1\nmetadata:\n    name: a\n";
    let tidy = jc_core::yaml_fmt::format(messy).expect("formats");
    assert_ne!(tidy, messy);

    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.CommitFiles",
        ))
        .and(body_json(json!({
            "Branch": "main",
            "Message": "create policy",
            "Author": { "Name": "Alice Developer", "Email": "alice@example.com" },
            "Committer": { "Name": "Alice Developer", "Email": "alice@example.com" },
            "Files": [{
                "operation": "upload",
                "path": "policies/a.yaml",
                "content": STANDARD.encode(&tidy)
            }]
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "Sha": "commit-sha-tidy"
        })))
        .mount(&server)
        .await;

    let write = FileWrite {
        path: "policies/a.yaml",
        branch: "main",
        message: "create policy",
        content: messy,
        sha: None,
        author: Author {
            name: "Alice Developer",
            email: "alice@example.com",
        },
    };
    assert_eq!(client.put_file(&write).await.unwrap(), "commit-sha-tidy");
}

/// AG-17, AG-137, T-5609: a commit and a merge request opened inside a call over MCP name the
/// channel and the client, so the approver and the forge history tell an AI client's proposal
/// from the person's own.
#[tokio::test]
async fn a_call_over_mcp_signs_its_commit_and_its_merge_request() {
    use joinedcontext_portal::change::{ProposedVia, PROPOSED_VIA};
    use wiremock::matchers::body_partial_json;

    let server = MockServer::start().await;
    let client = ShakoClient::new(
        server.uri().parse().unwrap(),
        "test-owner",
        "test-repo",
        "secret-token",
    )
    .unwrap();
    Mock::given(method("POST"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.CommitFiles",
        ))
        .and(body_string_contains(r#""path":"a.yaml""#))
        .and(body_partial_json(json!({
            "Message": "Update Endpoint a\n\nProposed-Via: mcp; client=mcp-mobile"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "Sha": "c1" })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/odata/Repositories(Owner='test-owner',Name='test-repo')/Shako.OpenPullRequest"))
        .and(body_partial_json(json!({
            "Body": "Proposed through an AI client over MCP (client `mcp-mobile`).\n\nPR description\n\nProposed-Via: mcp; client=mcp-mobile"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "Number": 18,
            "HtmlUrl": "https://shako.example.com/test-owner/test-repo/pulls/18",
            "State": "open",
            "Body": "x\n\nProposed-Via: mcp; client=mcp-mobile",
            "Merged": false
        })))
        .expect(1)
        .mount(&server)
        .await;

    let write = FileWrite {
        path: "a.yaml",
        branch: "feature-1",
        message: "Update Endpoint a",
        content: "a: 1\n",
        sha: None,
        author: Author {
            name: "Alice Developer",
            email: "alice@example.com",
        },
    };
    let pr = PROPOSED_VIA
        .scope(ProposedVia::mcp(Some("mcp-mobile")), async {
            client.put_file(&write).await.unwrap();
            client
                .create_pull_request("feature-1", "main", "Add feature", "PR description")
                .await
                .unwrap()
        })
        .await;
    assert_eq!(
        ProposedVia::in_body(&pr.body).map(|via| via.client),
        Some("mcp-mobile".to_owned())
    );
}
