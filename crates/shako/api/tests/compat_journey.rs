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
    let (st, err) = send(&app, "GET", "/odata/NoSuchSet", READER, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{err}");
}
