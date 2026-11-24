#![forbid(unsafe_code)]
//! One repository's life through the Gitea-compatible REST surface, the way a Gitea client
//! drives it: files, branches, statuses, trees, commits, tags, branch rules, pull requests with
//! reviews and a merge, push mirrors, an archive, then the repository's deletion.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use shako_api::{ShakoState, compat_and_native_rest_router, odata_router};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, Principal,
    RoleBinding, ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_store_memory::MemoryStore;
use std::sync::Arc;
use tower::ServiceExt;

const ADMIN: &str = "admin-token";
const READER: &str = "reader-token";
const APPROVER: &str = "approver-token";
const R: &str = "/api/v1/repos/joinedcontext/site";

fn world(root: &std::path::Path) -> Router {
    let org = OrgId::from_u128(10);
    let binding = |user: &str, actions| RoleBinding {
        org_id: org,
        principal: Principal::User(user.into()),
        actions,
        project_id: None,
        repo: None,
    };
    let authz = Arc::new(LocalAuthorizer::new(vec![
        binding(
            "admin",
            vec![
                Action::Admin,
                Action::Write,
                Action::Approve,
                Action::Publish,
            ],
        ),
        binding("reader", vec![Action::Read]),
        binding("approver", vec![Action::Write, Action::Approve]),
    ]));
    let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
        .with_static_tokens(vec![
            StaticTokenBinding::new(ADMIN, Subject::authenticated("admin", "admin", vec![org])),
            StaticTokenBinding::new(
                READER,
                Subject::authenticated("reader", "reader", vec![org]),
            ),
            StaticTokenBinding::new(
                APPROVER,
                Subject::authenticated("approver", "approver", vec![org]),
            ),
        ]);
    let guard = OutboundGuard::new(OutboundPolicy {
        allowed_hosts: Some(vec!["github.com".into()]),
        mock_handler: Some(Arc::new(|method, url, _headers, _body| {
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
        root.to_path_buf(),
        Arc::new(MemoryStore::new()),
        authz,
        verifier,
        guard,
    );
    compat_and_native_rest_router()
        .merge(odata_router())
        .with_state(state)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: &str,
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
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "bytes": bytes.len(), "head": bytes.iter().take(4).copied().collect::<Vec<u8>>() }));
    (status, value)
}

fn put(content: &str, message: &str, branch: &str) -> Value {
    json!({ "content": STANDARD.encode(content), "message": message, "branch": branch,
        "author": { "name": "Ada", "email": "ada@city.example" } })
}

#[tokio::test]
async fn a_repository_lives_through_the_compat_surface() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = world(tmp.path());
    let (st, repo) = send(&app, "POST", "/api/v1/orgs/joinedcontext/repos", ADMIN,
        Some(json!({ "name": "site", "auto_init": true, "default_branch": "main", "description": "d" }))).await;
    assert_eq!(st, StatusCode::CREATED, "{repo}");
    assert_eq!(repo["name"], json!("site"));
    let (st, _) = send(
        &app,
        "POST",
        "/api/v1/orgs/joinedcontext/repos",
        ADMIN,
        Some(json!({ "name": "site" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "a second repository of the name");

    // Files.
    let (st, file) = send(
        &app,
        "PUT",
        &format!("{R}/contents/docs/a.txt"),
        ADMIN,
        Some(put("hello", "add a", "main")),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{file}");
    assert_eq!(file["content"]["path"], json!("docs/a.txt"));
    let (st, got) = send(
        &app,
        "GET",
        &format!("{R}/contents/docs/a.txt?ref=main"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{got}");
    let blob = got["sha"]
        .as_str()
        .unwrap_or_else(|| panic!("{got}"))
        .to_owned();
    assert_eq!(
        STANDARD
            .decode(got["content"].as_str().unwrap_or_default())
            .ok(),
        Some(b"hello".to_vec()),
        "{got}"
    );
    let (st, _) = send(
        &app,
        "PUT",
        &format!("{R}/contents/docs/a.txt"),
        READER,
        Some(put("x", "m", "main")),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "a reader writes nothing");

    // Branches.
    let (st, branch) = send(
        &app,
        "POST",
        &format!("{R}/branches"),
        ADMIN,
        Some(json!({ "new_branch_name": "feature", "old_branch_name": "main" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{branch}");
    let (st, branch) = send(&app, "GET", &format!("{R}/branches/feature"), READER, None).await;
    assert_eq!(st, StatusCode::OK, "{branch}");
    assert_eq!(branch["name"], json!("feature"));
    let (st, _) = send(&app, "GET", &format!("{R}/branches/nowhere"), READER, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, file) = send(
        &app,
        "PUT",
        &format!("{R}/contents/docs/b.txt"),
        ADMIN,
        Some(put("bee", "add b", "feature")),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{file}");
    let head = file["commit"]["sha"]
        .as_str()
        .unwrap_or_else(|| panic!("{file}"))
        .to_owned();

    // Trees, commits, tags.
    let (st, tree) = send(
        &app,
        "GET",
        &format!("{R}/git/trees/feature?recursive=true"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{tree}");
    let paths: Vec<&str> = tree["tree"]
        .as_array()
        .unwrap_or_else(|| panic!("{tree}"))
        .iter()
        .filter_map(|e| e["path"].as_str())
        .collect();
    assert!(
        paths.contains(&"docs/b.txt") && paths.contains(&"docs/a.txt"),
        "{paths:?}"
    );
    let (st, tree) = send(
        &app,
        "GET",
        &format!("{R}/git/trees/main?page=1&per_page=1"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{tree}");
    let (st, _) = send(&app, "GET", &format!("{R}/git/trees/nowhere"), READER, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, commits) = send(
        &app,
        "GET",
        &format!("{R}/commits?sha=feature&limit=1"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{commits}");
    assert_eq!(commits[0]["sha"].as_str(), Some(head.as_str()));
    let (st, commit) = send(
        &app,
        "GET",
        &format!("{R}/git/commits/{head}"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{commit}");
    assert_eq!(commit["sha"].as_str(), Some(head.as_str()));
    let (st, _) = send(
        &app,
        "GET",
        &format!("{R}/git/commits/{}", "1".repeat(40)),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, tags) = send(&app, "GET", &format!("{R}/tags?limit=5"), READER, None).await;
    assert_eq!((st, tags), (StatusCode::OK, json!([])));

    // Commit statuses.
    let (st, status) = send(
        &app,
        "POST",
        &format!("{R}/statuses/{head}"),
        ADMIN,
        Some(json!({ "state": "success", "context": "ci", "description": "green" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{status}");
    let (st, combined) = send(
        &app,
        "GET",
        &format!("{R}/commits/{head}/status"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{combined}");
    assert_eq!(combined["state"], json!("success"));

    // Branch rules.
    let (st, rule) = send(
        &app,
        "POST",
        &format!("{R}/branch_protections"),
        ADMIN,
        Some(json!({ "rule_name": "main", "required_approvals": 1 })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{rule}");
    let (st, rules) = send(
        &app,
        "GET",
        &format!("{R}/branch_protections"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rules.as_array().map(Vec::len), Some(1), "{rules}");
    let (st, rule) = send(
        &app,
        "GET",
        &format!("{R}/branch_protections/main"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{rule}");
    let (st, rule) = send(
        &app,
        "PATCH",
        &format!("{R}/branch_protections/main"),
        ADMIN,
        Some(json!({ "required_approvals": 1 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{rule}");
    let (st, _) = send(
        &app,
        "GET",
        &format!("{R}/branch_protections/none"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // A pull request, its files, a review and the merge.
    let (st, pull) = send(
        &app,
        "POST",
        &format!("{R}/pulls"),
        ADMIN,
        Some(json!({ "title": "Add b", "head": "feature", "base": "main" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{pull}");
    let number = pull["number"].as_u64().unwrap_or_else(|| panic!("{pull}"));
    let (st, pulls) = send(
        &app,
        "GET",
        &format!("{R}/pulls?state=open&limit=10"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(pulls.as_array().map(Vec::len), Some(1), "{pulls}");
    let (st, files) = send(
        &app,
        "GET",
        &format!("{R}/pulls/{number}/files"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{files}");
    assert_eq!(files[0]["filename"], json!("docs/b.txt"));
    let (st, own) = send(
        &app,
        "POST",
        &format!("{R}/pulls/{number}/reviews"),
        ADMIN,
        Some(json!({ "event": "APPROVED", "body": "mine" })),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{own}");
    let (st, review) = send(
        &app,
        "POST",
        &format!("{R}/pulls/{number}/reviews"),
        APPROVER,
        Some(json!({ "event": "APPROVED", "body": "ok" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{review}");
    let (st, reviews) = send(
        &app,
        "GET",
        &format!("{R}/pulls/{number}/reviews"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(reviews.as_array().map(Vec::len), Some(1), "{reviews}");
    let (st, merged) = send(
        &app,
        "POST",
        &format!("{R}/pulls/{number}/merge"),
        ADMIN,
        Some(json!({ "Do": "merge" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{merged}");
    let (st, pulls) = send(
        &app,
        "GET",
        &format!("{R}/pulls?state=closed"),
        READER,
        None,
    )
    .await;
    assert_eq!(
        (st, pulls.as_array().map(Vec::len)),
        (StatusCode::OK, Some(1))
    );
    let (st, _) = send(
        &app,
        "DELETE",
        &format!("{R}/branch_protections/main"),
        ADMIN,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // Delete a file, then the merged branch.
    let (st, gone) = send(
        &app,
        "DELETE",
        &format!("{R}/contents/docs/a.txt"),
        ADMIN,
        Some(json!({ "message": "drop a", "branch": "main", "sha": blob })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{gone}");
    let (st, _) = send(
        &app,
        "GET",
        &format!("{R}/contents/docs/a.txt"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send(
        &app,
        "DELETE",
        &format!("{R}/branches/feature"),
        ADMIN,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // Push mirrors.
    let (st, mirror) = send(&app, "POST", &format!("{R}/push_mirrors"), ADMIN,
        Some(json!({ "remote_address": "https://github.com/joinedcontext-apps/site.git", "interval": "8h0m0s", "sync_on_commit": true }))).await;
    assert_eq!(st, StatusCode::OK, "{mirror}");
    let name = mirror["remote_name"]
        .as_str()
        .unwrap_or_else(|| panic!("{mirror}"))
        .to_owned();
    let (st, _) = send(&app, "GET", &format!("{R}/push_mirrors"), READER, None).await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "a mirror's target is the admin's to read"
    );
    let (st, mirrors) = send(&app, "GET", &format!("{R}/push_mirrors"), ADMIN, None).await;
    assert_eq!(
        (st, mirrors.as_array().map(Vec::len)),
        (StatusCode::OK, Some(1)),
        "{mirrors}"
    );
    let (st, sync) = send(&app, "POST", &format!("{R}/push_mirrors-sync"), ADMIN, None).await;
    assert_eq!(st, StatusCode::OK, "{sync}");
    let (st, _) = send(
        &app,
        "DELETE",
        &format!("{R}/push_mirrors/{name}"),
        ADMIN,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = send(
        &app,
        "POST",
        &format!("{R}/push_mirrors"),
        ADMIN,
        Some(json!({ "remote_address": "https://evil.example/site.git" })),
    )
    .await;
    assert!(
        st.is_client_error(),
        "a mirror outside the allowed hosts is refused: {st}"
    );

    // The archive, the repository's edit and deletion.
    let (st, zip) = send(&app, "GET", &format!("{R}/archive/main.zip"), READER, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(zip["head"], json!([0x50, 0x4b, 3, 4]), "a zip");
    let (st, repo) = send(
        &app,
        "PATCH",
        R,
        ADMIN,
        Some(json!({ "description": "new", "archived": false })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{repo}");
    assert_eq!(repo["description"], json!("new"));
    let (st, _) = send(&app, "DELETE", R, READER, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = send(&app, "DELETE", R, ADMIN, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = send(&app, "GET", R, READER, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// Every entity set the service document names answers a page to a reader, after a repository
/// has a branch, a commit status, a pull request with a review and a branch rule; an anonymous
/// caller reads none of it.
async fn op(app: &Router, name: &str, token: &str, body: Value) -> (StatusCode, Value) {
    send(app, "POST", &format!("/odata/{name}"), token, Some(body)).await
}

/// Every OData action and function a client, MCP or the CLI calls by name: each runs through the
/// same service as the REST surface, refuses a missing or malformed parameter with 422 naming
/// it, and a reader writes nothing.
#[tokio::test]
async fn every_odata_operation_runs_and_refuses_bad_parameters() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = world(tmp.path());
    const K: &str = "joinedcontext/ops";
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;

    let (st, repo) = op(
        &app,
        "CreateRepository",
        ADMIN,
        json!({ "Owner": "joinedcontext", "Name": "ops", "Description": "d", "Private": false }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{repo}");
    let rid = repo["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("{repo}"))
        .to_owned();
    let (st, _) = op(
        &app,
        "CreateRepository",
        ADMIN,
        json!({ "Owner": "joinedcontext" }),
    )
    .await;
    assert_eq!(st, unprocessable, "Name required");
    let (st, _) = op(
        &app,
        "Shako.CreateRepository",
        READER,
        json!({ "Owner": "joinedcontext", "Name": "nope" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "a reader creates nothing");

    // Branches and files.
    let (st, b) = op(
        &app,
        "CreateBranch",
        ADMIN,
        json!({ "repo": K, "NewBranchName": "feature", "OldBranchName": "main" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b["Name"], json!("feature"));
    let (st, _) = op(&app, "CreateBranch", ADMIN, json!({ "NewBranchName": "x" })).await;
    assert_eq!(st, unprocessable, "bound to a repository");
    let (st, _) = op(
        &app,
        "CreateBranch",
        ADMIN,
        json!({ "repo": "joinedcontext/none", "NewBranchName": "x" }),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let files =
        json!([{ "operation": "create", "path": "docs/a.txt", "content": STANDARD.encode("a") }]);
    let author = json!({ "Name": "Ada", "Email": "ada@city.example" });
    let (st, c) = op(
        &app,
        "CommitFiles",
        ADMIN,
        json!({ "repo": K, "Branch": "feature",
        "Message": "add a", "Files": files, "Author": author, "Committer": author }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{c}");
    assert!(c["Sha"].as_str().is_some_and(|s| s.len() == 40), "{c}");
    for (body, says) in [
        (json!({ "repo": K, "Message": "m" }), "Files required"),
        (
            json!({ "repo": K, "Message": "m", "Files": "nope" }),
            "invalid Files",
        ),
        (json!({ "repo": K, "Files": [] }), "Message required"),
        (
            json!({ "repo": K, "Message": "m", "Files": [], "Author": { "Name": "Ada" } }),
            "Author takes Name and Email",
        ),
    ] {
        let (st, e) = op(&app, "CommitFiles", ADMIN, body).await;
        assert_eq!(st, unprocessable, "{e}");
        assert!(e.to_string().contains(says), "{says}: {e}");
    }

    // Functions read the repository under lower- or upper-case argument names.
    let (st, tree) = op(
        &app,
        "Tree",
        READER,
        json!({ "repo": K, "ref": "feature", "recursive": true }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{tree}");
    assert!(tree.to_string().contains("docs/a.txt"), "{tree}");
    let (st, file) = op(
        &app,
        "File",
        READER,
        json!({ "repo": K, "Ref": "feature", "Path": "docs/a.txt" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{file}");
    let (st, _) = op(&app, "File", READER, json!({ "repo": K, "Ref": "feature" })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "File requires Path");
    let (st, hist) = op(
        &app,
        "History",
        READER,
        json!({ "repo": K, "ref": "feature", "top": 1 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{hist}");
    let (st, cmp) = op(
        &app,
        "Compare",
        READER,
        json!({ "repo": K, "base": "main", "head": "feature" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{cmp}");
    let sha = c["Sha"].as_str().unwrap_or_default().to_owned();
    let (st, cs) = op(
        &app,
        "CommitStatus",
        READER,
        json!({ "repo": K, "sha": sha }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{cs}");
    let (st, _) = op(
        &app,
        "CommitStatus",
        READER,
        json!({ "repo": K, "Ref": "feature" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "CommitStatus requires Sha");

    // A pull request's life through the actions.
    let (st, pr) = op(
        &app,
        "OpenPullRequest",
        ADMIN,
        json!({ "repo": K, "Head": "feature", "Title": "t" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pr}");
    let number = pr["Number"].as_u64().unwrap_or_else(|| panic!("{pr}"));
    let (st, _) = op(
        &app,
        "OpenPullRequest",
        ADMIN,
        json!({ "repo": K, "Head": "feature" }),
    )
    .await;
    assert_eq!(st, unprocessable, "Title required");
    let (st, _) = op(
        &app,
        "Review",
        APPROVER,
        json!({ "repo": K, "Number": number, "Event": "juggle" }),
    )
    .await;
    assert_eq!(st, unprocessable, "an unknown verdict");
    let (st, _) = op(
        &app,
        "Review",
        APPROVER,
        json!({ "repo": K, "Event": "APPROVED" }),
    )
    .await;
    assert_eq!(st, unprocessable, "Number required");
    let (st, rv) = op(
        &app,
        "Review",
        APPROVER,
        json!({ "repo": format!("{K}:{number}"), "Event": "APPROVED", "Body": "ok" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{rv}");
    let (st, m) = op(
        &app,
        "Mergeable",
        READER,
        json!({ "repo": K, "Number": number }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{m}");
    let (st, _) = op(
        &app,
        "UpdateBranch",
        ADMIN,
        json!({ "repo": K, "Number": number, "Style": "rebase" }),
    )
    .await;
    assert_eq!(st, unprocessable, "only a merge update");
    let (st, up) = op(
        &app,
        "UpdateBranch",
        ADMIN,
        json!({ "repo": K, "Number": number }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{up}");
    let (st, _) = op(
        &app,
        "Merge",
        ADMIN,
        json!({ "repo": K, "Number": number, "Style": "juggle" }),
    )
    .await;
    assert_eq!(st.as_u16() / 100, 4, "an unknown merge style");
    let (st, merged) = op(
        &app,
        "Merge",
        ADMIN,
        json!({ "repo": K, "Number": number, "Style": "squash",
        "Message": "m", "DeleteBranchAfterMerge": true, "Author": author }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{merged}");
    assert_eq!(merged["Merged"], json!(true));
    let (st, b2) = op(
        &app,
        "CreateBranch",
        ADMIN,
        json!({ "repo": K, "NewBranchName": "second" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b2}");
    let (_, _) = op(
        &app,
        "CommitFiles",
        ADMIN,
        json!({ "repo": K, "Branch": "second", "Message": "b",
        "Files": [{ "operation": "create", "path": "b.txt", "content": STANDARD.encode("b") }] }),
    )
    .await;
    let (st, pr2) = op(
        &app,
        "OpenPullRequest",
        ADMIN,
        json!({ "repo": K, "Head": "second", "Base": "main", "Title": "two", "Body": "b" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pr2}");
    let n2 = pr2["Number"].as_u64().unwrap_or_default();
    let (st, closed) = op(&app, "Close", ADMIN, json!({ "repo": K, "Number": n2 })).await;
    assert_eq!(st, StatusCode::OK, "{closed}");
    let (st, _) = op(
        &app,
        "DeleteBranch",
        ADMIN,
        json!({ "repo": K, "Branch": "second" }),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = op(&app, "DeleteBranch", ADMIN, json!({ "repo": K })).await;
    assert_eq!(st, unprocessable, "Branch required");

    // Links to an archive or a bundle of a ref.
    let (st, link) = op(
        &app,
        "ArchiveLink",
        READER,
        json!({ "repo": K, "Format": "tar.gz" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{link}");
    assert!(
        link["value"]
            .as_str()
            .is_some_and(|v| v.ends_with("/archive/main.tar.gz")),
        "{link}"
    );
    let (st, link) = op(
        &app,
        "BundleLink",
        READER,
        json!({ "repo": K, "Ref": "main" }),
    )
    .await;
    assert!(
        st == StatusCode::OK
            && link["value"]
                .as_str()
                .is_some_and(|v| v.ends_with("main.bundle")),
        "{link}"
    );
    let (st, _) = op(
        &app,
        "ArchiveLink",
        READER,
        json!({ "repo": K, "Format": "rar" }),
    )
    .await;
    assert_eq!(st, unprocessable, "zip or tar.gz");
    let (st, _) = op(
        &app,
        "ArchiveLink",
        READER,
        json!({ "repo": K, "Ref": "../x" }),
    )
    .await;
    assert_eq!(st.as_u16() / 100, 4, "a ref that names no branch or tag");

    // Branch rules.
    let (st, rule) = op(
        &app,
        "Protect",
        ADMIN,
        json!({ "repo": K, "Rule": { "rule_name": "main", "required_approvals": 1 } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{rule}");
    assert_eq!(rule["Pattern"], json!("main"));
    let (st, _) = op(
        &app,
        "Protect",
        ADMIN,
        json!({ "repo": K, "Rule": { "rule_name": "main", "nope": 1 } }),
    )
    .await;
    assert_eq!(st, unprocessable, "an unknown rule field");
    let (st, _) = op(
        &app,
        "Unprotect",
        ADMIN,
        json!({ "repo": K, "Pattern": "main" }),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = op(&app, "Unprotect", ADMIN, json!({ "repo": K })).await;
    assert_eq!(st, unprocessable, "Pattern required");

    // Push mirrors through the outbound guard's github.com.
    let (st, mirror) = op(&app, "AddPushMirror", ADMIN, json!({ "repo": K,
        "RemoteAddress": "https://github.com/joinedcontext/ops.git", "Interval": "8h0m0s", "SyncOnCommit": false })).await;
    assert_eq!(st, StatusCode::OK, "{mirror}");
    let (st, _) = op(&app, "AddPushMirror", ADMIN, json!({ "repo": K })).await;
    assert_eq!(st, unprocessable, "RemoteAddress required");
    let (st, _) = op(&app, "SyncPushMirrors", ADMIN, json!({ "repo": K })).await;
    assert!(st.is_success(), "{st}");
    let mirror_name = mirror["Name"].as_str().unwrap_or_default().to_owned();
    assert!(mirror_name.starts_with("mirror-"), "{mirror}");
    let (st, _) = op(
        &app,
        "RemovePushMirror",
        ADMIN,
        json!({ "repo": K, "Name": mirror_name }),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{mirror}");
    let (st, _) = op(&app, "RemovePushMirror", ADMIN, json!({ "repo": K })).await;
    assert_eq!(st, unprocessable, "Name required");

    // Webhooks.
    for (body, says) in [
        (
            json!({ "repo": K, "Url": "https://github.com/hook" }),
            "Secret or SecretRef",
        ),
        (
            json!({ "repo": K, "Url": "https://github.com/hook", "Secret": "s", "SecretRef": "r" }),
            "Secret or SecretRef",
        ),
        (
            json!({ "repo": K, "Url": "https://github.com/hook", "Secret": "s", "Events": 3 }),
            "Events is a list",
        ),
        (json!({ "repo": K, "Secret": "s" }), "Url required"),
    ] {
        let (st, e) = op(&app, "AddWebhook", ADMIN, body).await;
        assert_eq!(st, unprocessable, "{e}");
        assert!(e.to_string().contains(says), "{says}: {e}");
    }
    let (st, hook) = op(
        &app,
        "AddWebhook",
        ADMIN,
        json!({ "repo": K, "Url": "https://github.com/hook",
        "Secret": "hook-secret", "Events": ["push"], "Active": true }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{hook}");
    assert!(
        !hook.to_string().contains("hook-secret"),
        "the secret is never answered"
    );
    let hook_id = hook["Id"].as_str().unwrap_or_default().to_owned();
    let (st, _) = op(
        &app,
        "RedeliverWebhook",
        ADMIN,
        json!({ "Id": "not-a-uuid" }),
    )
    .await;
    assert_eq!(st, unprocessable);
    let (st, _) = op(
        &app,
        "RedeliverWebhook",
        ADMIN,
        json!({ "Id": "00000000-0000-4000-8000-000000000000" }),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, del) = op(&app, "RedeliverWebhook", ADMIN, json!({ "Id": hook_id })).await;
    assert_eq!(st, StatusCode::OK, "{del}");

    // Generic packages.
    let (st, pkg) = op(
        &app,
        "PublishPackage",
        ADMIN,
        json!({ "Owner": "joinedcontext", "Package": "tool",
        "Version": "1.0.0", "File": "tool.txt", "ContentBase64": STANDARD.encode("bytes") }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pkg}");
    assert_eq!(pkg["Size"], json!(5));
    let (st, _) = op(
        &app,
        "PublishPackage",
        ADMIN,
        json!({ "Owner": "joinedcontext", "Package": "tool",
        "Version": "1.0.1", "File": "t", "ContentBase64": "*not base64*" }),
    )
    .await;
    assert_eq!(st, unprocessable, "invalid ContentBase64");

    // Who asks and what they may do.
    let (st, me) = op(&app, "Me", READER, json!({})).await;
    assert_eq!(st, StatusCode::OK, "{me}");
    for (object, allowed) in [
        (json!(null), true),
        (json!(format!("Repository:{K}")), true),
        (json!(rid.clone()), true),
        (json!("ops"), true),
        (json!("joinedcontext/none"), false),
    ] {
        let (st, dec) = op(
            &app,
            "Allowed",
            READER,
            json!({ "Operation": "Read", "Object": object }),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{dec}");
        assert_eq!(
            dec["allowed"].as_bool().or(dec["Allowed"].as_bool()),
            Some(allowed),
            "{object}: {dec}"
        );
    }
    let (st, _) = op(&app, "Juggle", READER, json!({})).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "an unknown operation");

    // A copy, then the archive and deletion of the original.
    let (st, copy) = op(
        &app,
        "ImportRepository",
        ADMIN,
        json!({ "SourceOwner": "joinedcontext", "SourceName": "ops",
        "RepoOwner": "joinedcontext", "RepoName": "ops-copy" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{copy}");
    let (st, _) = op(
        &app,
        "ImportRepository",
        ADMIN,
        json!({ "SourceOwner": "joinedcontext" }),
    )
    .await;
    assert_eq!(st, unprocessable, "SourceName required");
    let (st, arch) = op(&app, "Archive", ADMIN, json!({ "repo": K })).await;
    assert_eq!(st, StatusCode::OK, "{arch}");
    let (st, _) = op(&app, "Delete", READER, json!({ "repo": K })).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = op(&app, "Delete", ADMIN, json!({ "repo": K })).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = op(&app, "Archive", ADMIN, json!({ "repo": K })).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "gone");
}

/// OData reads by key, by bare name and through expansions, the package sets, the set-read tools
/// MCP calls, and the refusals of a key or path that names nothing.
#[tokio::test]
async fn odata_reads_by_key_name_and_expansion_and_refuse_what_names_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = world(tmp.path());
    const K: &str = "joinedcontext/ops";
    const NONE: &str = "00000000-0000-4000-8000-000000000000";
    let get = |uri: String| {
        let app = app.clone();
        async move { send(&app, "GET", &uri, READER, None).await }
    };
    let (st, repo) = op(
        &app,
        "CreateRepository",
        ADMIN,
        json!({ "Owner": "joinedcontext", "Name": "ops" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{repo}");
    let rid = repo["Id"].as_str().unwrap_or_default().to_owned();
    let (_, _) = op(
        &app,
        "CreateBranch",
        ADMIN,
        json!({ "repo": K, "NewBranchName": "feature" }),
    )
    .await;
    let (st, c) = op(
        &app,
        "CommitFiles",
        ADMIN,
        json!({ "repo": K, "Branch": "feature", "Message": "a",
        "Files": [{ "operation": "create", "path": "a.txt", "content": STANDARD.encode("a") }] }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{c}");
    let sha = c["Sha"].as_str().unwrap_or_default().to_owned();
    let (st, pr) = op(
        &app,
        "OpenPullRequest",
        ADMIN,
        json!({ "repo": K, "Head": "feature", "Title": "t" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pr}");
    let (st, status) = send(
        &app,
        "POST",
        &format!("/api/v1/repos/{K}/statuses/{sha}"),
        ADMIN,
        Some(json!({ "state": "success", "context": "ci", "description": "green" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{status}");
    let (st, pkg) = op(
        &app,
        "PublishPackage",
        ADMIN,
        json!({ "Package": "tool", "Version": "1.0.0",
        "File": "tool.txt", "ContentBase64": STANDARD.encode("bytes") }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pkg}");
    assert_eq!(
        pkg["Owner"],
        json!("admin"),
        "the caller owns a package that names no owner"
    );

    // Keys: named, by id, by bare name, unknown and ambiguous.
    let (st, one) = get(format!("/odata/Repositories(Id='{rid}')")).await;
    assert_eq!(
        (st, one["Name"].clone()),
        (StatusCode::OK, json!("ops")),
        "{one}"
    );
    let (st, _) = get(format!("/odata/Repositories('{NONE}')")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, one) = get("/odata/Repositories('ops')".into()).await;
    assert_eq!(
        (st, one["Id"].clone()),
        (StatusCode::OK, json!(rid)),
        "{one}"
    );
    let (st, _) = get("/odata/Repositories('nothing')".into()).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, mine) = op(&app, "CreateRepository", ADMIN, json!({ "Name": "ops" })).await;
    assert_eq!(
        (st, mine["Owner"].clone()),
        (StatusCode::OK, json!("admin")),
        "{mine}"
    );
    let (st, e) = send(&app, "GET", "/odata/Repositories('ops')", ADMIN, None).await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a bare name two owners hold: {e}"
    );

    // A repository the caller cannot see, or none, answers an empty set; a bad ref is refused.
    for set in ["Branches", "Commits", "Tags"] {
        let (st, page) = get(format!("/odata/{set}?$filter=RepoId%20eq%20'{NONE}'")).await;
        assert_eq!(st, StatusCode::OK, "{set}: {page}");
        assert_eq!(page["value"], json!([]), "{set}");
    }
    let (st, _) = get(format!("/odata/Commits('{rid}:..bad')")).await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a ref that is no branch, tag or id"
    );
    let (st, _) = get("/odata/Nope".into()).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send(&app, "DELETE", "/odata/Branches('main')", ADMIN, None).await;
    assert_eq!(st, StatusCode::METHOD_NOT_ALLOWED);

    // Pull request expansions.
    let (st, _) = get("/odata/PullRequests?$expand=Nope".into()).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = get("/odata/Repositories?$expand=Files".into()).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "only pull requests expand");
    let (st, page) = get("/odata/PullRequests?$expand=Files,,".into()).await;
    assert_eq!(st, StatusCode::OK, "{page}");
    assert!(
        page["value"][0]["Files"]
            .as_array()
            .is_some_and(|f| !f.is_empty()),
        "{page}"
    );
    assert!(page["value"][0].get("Reviews").is_none(), "{page}");
    let (st, page) = get("/odata/PullRequests?$expand=Reviews".into()).await;
    assert_eq!(st, StatusCode::OK, "{page}");
    assert_eq!(page["value"][0]["Reviews"], json!([]), "{page}");
    assert!(page["value"][0].get("Files").is_none(), "{page}");
    for select in ["Title", "RepoId"] {
        let (st, page) = get(format!(
            "/odata/PullRequests?$expand=Reviews&$select={select}"
        ))
        .await;
        assert_eq!(st, StatusCode::OK, "{page}");
        assert!(
            page["value"][0].get("Reviews").is_none(),
            "a row without its keys expands nothing: {page}"
        );
    }
    let (st, _) = get(format!("/odata/PullRequests('{rid}:abc')/Reviews")).await;
    assert_eq!(st.as_u16() / 100, 4, "a pull request key is its number");
    let (st, _) = get(format!(
        "/odata/Repositories('{rid}')/PullRequests('abc')/Files"
    ))
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "a pull request key is its number"
    );
    let (st, _) = get(format!("/odata/PullRequests('{rid}:999')/Mergeable")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Functions over the repository's files and statuses.
    let (st, _) = op(
        &app,
        "File",
        READER,
        json!({ "repo": K, "Ref": "feature", "Path": "none.txt" }),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, cs) = op(
        &app,
        "CommitStatus",
        READER,
        json!({ "repo": K, "Sha": sha }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{cs}");
    assert!(cs.to_string().contains("green"), "{cs}");

    // Packages.
    for set in ["Packages", "PackageVersions", "PackageFiles"] {
        let (st, page) = get(format!("/odata/{set}")).await;
        assert_eq!(st, StatusCode::OK, "{set}: {page}");
        assert_eq!(
            page["value"].as_array().map(Vec::len),
            Some(1),
            "{set}: {page}"
        );
    }
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/odata/PackageFiles(Owner='admin',Name='tool',Version='1.0.0',File='tool.txt')/$value")
                .header("authorization", format!("token {READER}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("answer");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16)
        .await
        .expect("body");
    assert_eq!(&bytes[..], b"bytes");
    let (st, _) = get("/odata/PackageFiles(Owner='admin',Name='tool')/$value".into()).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "the whole key");

    // The set-read tools narrow by repository and the caller's filter together.
    let (st, rows) = op(
        &app,
        "jc_forge_branches_list",
        READER,
        json!({ "repo": K, "Filter": "Name eq 'feature'" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{rows}");
    assert_eq!(rows["value"].as_array().map(Vec::len), Some(1), "{rows}");
    let (st, rows) = op(&app, "jc_forge_branches_list", READER, json!({ "repo": K })).await;
    assert_eq!(
        rows["value"].as_array().map(Vec::len),
        Some(2),
        "{st} {rows}"
    );
}

#[tokio::test]
async fn every_odata_set_answers_a_reader_and_refuses_the_anonymous() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = world(tmp.path());
    let (st, repo) = send(
        &app,
        "POST",
        "/api/v1/orgs/joinedcontext/repos",
        ADMIN,
        Some(json!({ "name": "site", "auto_init": true })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{repo}");
    send(
        &app,
        "POST",
        &format!("{R}/branches"),
        ADMIN,
        Some(json!({ "new_branch_name": "feature" })),
    )
    .await;
    let (_, file) = send(
        &app,
        "PUT",
        &format!("{R}/contents/a.txt"),
        ADMIN,
        Some(put("a", "add a", "feature")),
    )
    .await;
    let head = file["commit"]["sha"]
        .as_str()
        .unwrap_or_else(|| panic!("{file}"))
        .to_owned();
    send(
        &app,
        "POST",
        &format!("{R}/statuses/{head}"),
        ADMIN,
        Some(json!({ "state": "pending", "context": "ci" })),
    )
    .await;
    send(
        &app,
        "POST",
        &format!("{R}/branch_protections"),
        ADMIN,
        Some(json!({ "rule_name": "main" })),
    )
    .await;
    let (st, pull) = send(
        &app,
        "POST",
        &format!("{R}/pulls"),
        ADMIN,
        Some(json!({ "title": "a", "head": "feature", "base": "main" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{pull}");
    send(
        &app,
        "POST",
        &format!("{R}/pulls/1/reviews"),
        APPROVER,
        Some(json!({ "event": "COMMENT", "body": "looks fine" })),
    )
    .await;

    let (st, hook) = send(&app, "POST", &format!("{R}/hooks"), ADMIN, Some(json!({ "type": "gitea",
        "config": { "url": "https://github.com/joinedcontext-apps/hook", "content_type": "json", "secret": "hook-signing-value" },
        "events": ["push"], "active": true }))).await;
    assert_eq!(st, StatusCode::CREATED, "{hook}");
    let (st, mirror) = send(
        &app,
        "POST",
        &format!("{R}/push_mirrors"),
        ADMIN,
        Some(json!({ "remote_address": "https://github.com/joinedcontext-apps/site.git" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{mirror}");
    let (st, doc) = send(&app, "GET", "/odata/", READER, None).await;
    assert_eq!(st, StatusCode::OK, "{doc}");
    let sets: Vec<String> = doc["value"]
        .as_array()
        .unwrap_or_else(|| panic!("{doc}"))
        .iter()
        .filter_map(|s| s["name"].as_str().map(str::to_owned))
        .collect();
    for set in [
        "Repositories",
        "Branches",
        "Commits",
        "PullRequests",
        "Reviews",
        "ProtectionRules",
    ] {
        assert!(sets.iter().any(|s| s == set), "{set} in {sets:?}");
    }
    let (st, _) = send(&app, "GET", "/odata/$metadata", READER, None).await;
    assert_eq!(st, StatusCode::OK);
    let (_, repos) = send(&app, "GET", "/odata/Repositories", READER, None).await;
    let rid = repos["value"][0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("{repos}"))
        .to_owned();
    let mut rows = std::collections::BTreeMap::new();
    for set in &sets {
        let (mut st, mut page) = send(&app, "GET", &format!("/odata/{set}"), READER, None).await;
        if st == StatusCode::BAD_REQUEST || st == StatusCode::NOT_IMPLEMENTED {
            assert!(
                page["error"]["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("RepoId")),
                "{set}: {page}"
            );
            (st, page) = send(
                &app,
                "GET",
                &format!("/odata/{set}?$filter=RepoId%20eq%20'{rid}'"),
                READER,
                None,
            )
            .await;
        }
        assert_eq!(st, StatusCode::OK, "{set}: {page}");
        rows.insert(set.clone(), page["value"].as_array().map_or(0, Vec::len));
        let req = Request::builder()
            .uri(format!("/odata/{set}"))
            .body(Body::empty())
            .expect("request");
        let anon = app.clone().oneshot(req).await.expect("answer").status();
        assert_eq!(anon, StatusCode::UNAUTHORIZED, "{set}");
    }
    for (set, at_least) in [
        ("Repositories", 1),
        ("Branches", 2),
        ("Commits", 1),
        ("PullRequests", 1),
        ("Instances", 1),
        ("Reviews", 1),
        ("ProtectionRules", 1),
    ] {
        assert!(
            rows.get(set).copied().unwrap_or(0) >= at_least,
            "{set}: {rows:?}"
        );
    }
    let (st, page) = send(&app, "GET", &format!("/odata/Branches?$filter=RepoId%20eq%20'{rid}'%20and%20Name%20eq%20'feature'&$select=Name"), READER, None).await;
    assert_eq!(st, StatusCode::OK, "{page}");
    assert_eq!(page["value"].as_array().map(Vec::len), Some(1), "{page}");
    let (st, page) = send(
        &app,
        "GET",
        &format!("/odata/Repositories('{rid}')/Branches"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{page}");
    // A hook's and a mirror's targets are the admin's: a reader's page holds neither.
    for set in ["Webhooks", "PushMirrors"] {
        assert_eq!(rows.get(set), Some(&0), "{set}: {rows:?}");
        let (st, page) = send(
            &app,
            "GET",
            &format!("/odata/{set}?$filter=RepoId%20eq%20'{rid}'"),
            ADMIN,
            None,
        )
        .await;
        assert_eq!(
            (st, page["value"].as_array().map(Vec::len)),
            (StatusCode::OK, Some(1)),
            "{set}: {page}"
        );
    }
    let text = |app: &Router, uri: String| {
        let app = app.clone();
        async move {
            let req = Request::builder()
                .uri(uri)
                .header("authorization", format!("token {READER}"))
                .body(Body::empty())
                .expect("request");
            let resp = app.oneshot(req).await.expect("answer");
            let st = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .expect("body");
            (st, String::from_utf8_lossy(&bytes).into_owned())
        }
    };
    assert_eq!(
        text(
            &app,
            format!("/odata/Branches/$count?$filter=RepoId%20eq%20'{rid}'")
        )
        .await,
        (StatusCode::OK, "2".to_owned())
    );
    let (st, _) = text(
        &app,
        "/odata/Repositories/$count?$filter=Nope%20eq%201".to_owned(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "an unknown property");
    let (st, rows) = send(&app, "GET", "/odata/Repositories/$export", READER, None).await;
    assert_eq!(
        (st, rows.as_array().map(Vec::len)),
        (StatusCode::OK, Some(1)),
        "{rows}"
    );
    let (st, _) = send(
        &app,
        "GET",
        "/odata/Repositories/$export?$filter=Nope%20eq%201",
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, dec) = send(
        &app,
        "GET",
        "/odata/Repositories/$allowed(op='read')",
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{dec}");
    let (st, _) = send(
        &app,
        "GET",
        "/odata/Repositories/$allowed(op='juggle')",
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = send(&app, "GET", "/odata/Repositories/$nothing", READER, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send(&app, "GET", "/odata/Nope/$count", READER, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, pulls) = send(
        &app,
        "GET",
        &format!("/odata/PullRequests?$filter=RepoId%20eq%20'{rid}'"),
        READER,
        None,
    )
    .await;
    let pid = pulls["value"][0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("{pulls}"))
        .to_owned();
    let (st, merge) = send(
        &app,
        "GET",
        &format!("/odata/PullRequests('{pid}')/Mergeable"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{merge}");
    for (part, n) in [("Reviews", 1), ("Files", 1)] {
        let (st, page) = send(
            &app,
            "GET",
            &format!("/odata/PullRequests('{pid}')/{part}"),
            READER,
            None,
        )
        .await;
        assert_eq!(
            (st, page["value"].as_array().map(Vec::len)),
            (StatusCode::OK, Some(n)),
            "{part}: {page}"
        );
    }
    // A pull request's key is `<repository id>:<number>`.
    let (st, e) = send(
        &app,
        "GET",
        &format!("/odata/PullRequests('{rid}:9')/Files"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{e}");
    let (st, e) = send(
        &app,
        "GET",
        &format!("/odata/PullRequests('{rid}')/Files"),
        READER,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    let (st, file) = text(
        &app,
        format!("/odata/Repositories('{rid}')/File(path='a.txt',ref='feature')"),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{file}");
    let (st, _) = send(
        &app,
        "GET",
        "/odata/PackageFiles(Owner='joinedcontext')/$value",
        READER,
        None,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "a file key names all four parts"
    );
    let (st, err) = send(&app, "GET", "/odata/NoSuchSet", READER, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{err}");
}
