#![forbid(unsafe_code)]
//! The OData operations the Portal's `ShakoClient` calls (`PORTAL-ODATA.md`), one router in
//! process: repository alternate keys, navigation under a repository, parameter aliases, the
//! actions that write, native webhooks, packages. Each write is checked for auth (no token is 401),
//! organization isolation (another organization's repository is 404, the same body as a missing
//! one) and the verb (a reader is 403 on a visible repository).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use http_body_util::BodyExt;
use jc_router::Mount;
use serde_json::{Value, json};
use shako_api::verify_gitea_signature;
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_server::{ShakoState, router};
use shako_store_memory::MemoryStore;
use std::sync::Arc;
use tower::ServiceExt;

const HOOK_URL: &str = "https://portal.city.example/api/v1/webhooks/shako";
const HOOK_SECRET: &str = "odata-hook-secret-value";
const R: &str = "/odata/Repositories(Owner='joinedcontext',Name='helsinki')";

/// One delivery the outbound guard caught: its headers and body.
type Delivered = Arc<parking_lot::Mutex<Vec<(Vec<(String, String)>, Vec<u8>)>>>;

struct World {
    _tmp: tempfile::TempDir,
    state: ShakoState,
    app: Router,
    delivered: Delivered,
}

fn world() -> World {
    let tmp = tempfile::tempdir().expect("tempdir");
    let org_a = OrgId::from_u128(10);
    let org_b = OrgId::from_u128(20);
    let binding = |org_id, principal: &str, actions: Vec<Action>, repo: Option<&str>| RoleBinding {
        org_id,
        principal: principal.into(),
        actions,
        project_id: None,
        repo: repo.map(str::to_owned),
    };
    let authz = Arc::new(LocalAuthorizer::new(vec![
        binding(
            org_a,
            "jc-portal",
            vec![
                Action::Admin,
                Action::Write,
                Action::Approve,
                Action::Publish,
            ],
            None,
        ),
        binding(org_a, "reader-bob", vec![Action::Read], None),
        // Carol reads one repository of the organization and nothing else.
        binding(
            org_a,
            "scoped-carol",
            vec![Action::Read],
            Some("public-one"),
        ),
        binding(org_b, "admin-b", vec![Action::Admin, Action::Write], None),
    ]));
    let person = |login: &str, org| Subject::authenticated(login, login, vec![org], vec![]);
    let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
        .with_static_tokens(vec![
            StaticTokenBinding::new("portal-token", person("jc-portal", org_a)),
            StaticTokenBinding::new("bob-token", person("reader-bob", org_a)),
            StaticTokenBinding::new("carol-token", person("scoped-carol", org_a)),
            StaticTokenBinding::new("org-b-token", person("admin-b", org_b)),
        ]);
    let delivered: Delivered = Arc::default();
    let sink = delivered.clone();
    let guard = OutboundGuard::new(OutboundPolicy {
        allowed_hosts: Some(vec!["github.com".into(), "portal.city.example".into()]),
        allow_loopback_for_tests: false,
        mock_handler: Some(Arc::new(move |_method, url, headers, body| {
            if url == HOOK_URL {
                sink.lock().push((headers.to_vec(), body.to_vec()));
                return Some((202, Vec::new()));
            }
            None
        })),
        ..OutboundPolicy::default()
    });
    let state = ShakoState::new(
        InstanceId::from_u128(1),
        org_a,
        tmp.path().to_path_buf(),
        Arc::new(MemoryStore::new()),
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
    World {
        _tmp: tmp,
        state,
        app,
        delivered,
    }
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
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
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    let val = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
    (status, val, bytes)
}

fn b64(s: &str) -> String {
    STANDARD.encode(s.as_bytes())
}

async fn create_repo(w: &World, name: &str) {
    let (st, body, _) = call(
        &w.app,
        "POST",
        "/odata/CreateRepository",
        Some("portal-token"),
        Some(json!({"Owner": "joinedcontext", "Name": name, "Private": true, "AutoInit": true, "DefaultBranch": "main"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

async fn commit(w: &World, repo: &str, branch: &str, files: Value) -> (StatusCode, Value) {
    let (st, body, _) = call(
        &w.app,
        "POST",
        &format!("{repo}/Shako.CommitFiles"),
        Some("portal-token"),
        Some(json!({
            "Branch": branch,
            "Message": "Portal change\n\nwith a body line",
            "Author": {"Name": "Alice Steward", "Email": "alice@city.example"},
            "Committer": {"Name": "jc-portal", "Email": "portal@city.example"},
            "Files": files,
        })),
    )
    .await;
    (st, body)
}

async fn head_of(w: &World, branch: &str) -> String {
    let (st, body, _) = call(
        &w.app,
        "GET",
        &format!("{R}/Branches('{branch}')"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    body["HeadSha"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn repositories_by_alternate_key_create_conflict_import_archive_delete() {
    let w = world();
    create_repo(&w, "helsinki").await;
    let (st, _, _) = call(
        &w.app,
        "POST",
        "/odata/CreateRepository",
        Some("portal-token"),
        Some(json!({"Owner": "joinedcontext", "Name": "helsinki"})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "a second create is 409");

    let (st, repo, _) = call(&w.app, "GET", R, Some("portal-token"), None).await;
    assert_eq!(st, StatusCode::OK, "{repo}");
    assert_eq!(repo["DefaultBranch"], "main");
    assert_eq!(repo["Private"], true);
    // The UUID key names the same entity.
    let id = repo["Id"].as_str().unwrap();
    let (st, by_id, _) = call(
        &w.app,
        "GET",
        &format!("/odata/Repositories('{id}')"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(by_id["Name"], "helsinki");

    let (st, copy, _) = call(
        &w.app,
        "POST",
        "/odata/ImportRepository",
        Some("portal-token"),
        Some(json!({"SourceOwner": "joinedcontext", "SourceName": "helsinki", "RepoOwner": "joinedcontext", "RepoName": "helsinki-copy", "Private": true})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{copy}");
    assert_eq!(copy["DefaultBranch"], "main");
    let (st, _, _) = call(
        &w.app,
        "POST",
        "/odata/ImportRepository",
        Some("portal-token"),
        Some(json!({"SourceOwner": "joinedcontext", "SourceName": "helsinki", "RepoOwner": "joinedcontext", "RepoName": "helsinki-copy"})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "an existing import target is 409");

    let copy_r = "/odata/Repositories(Owner='joinedcontext',Name='helsinki-copy')";
    let (st, archived, _) = call(
        &w.app,
        "POST",
        &format!("{copy_r}/Shako.Archive"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(archived["Archived"], true);
    let (st, _, _) = call(&w.app, "DELETE", copy_r, Some("portal-token"), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _) = call(&w.app, "GET", copy_r, Some("portal-token"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn contents_history_tree_and_a_stale_sha_that_moves_nothing() {
    let w = world();
    create_repo(&w, "helsinki").await;
    let (st, first) = commit(
        &w,
        R,
        "main",
        json!([
            {"operation": "create", "path": "projects/a.yaml", "content": b64("a: 1\n")},
            {"operation": "create", "path": "projects/b.yaml", "content": b64("b: 1\n")},
        ]),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{first}");
    let first_sha = first["Sha"].as_str().unwrap().to_owned();
    assert_eq!(head_of(&w, "main").await, first_sha);

    // A path with `/` travels as a parameter alias.
    let (st, file, _) = call(
        &w.app,
        "GET",
        &format!("{R}/Shako.File(Path=@p,Ref=@r)?@p='projects/a.yaml'&@r='main'"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{file}");
    assert_eq!(
        STANDARD.decode(file["Content"].as_str().unwrap()).unwrap(),
        b"a: 1\n"
    );
    let blob_sha = file["Sha"].as_str().unwrap().to_owned();
    let (st, _, _) = call(
        &w.app,
        "GET",
        &format!("{R}/Shako.File(Path=@p,Ref=@r)?@p='projects/none.yaml'&@r='main'"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "an absent file is 404");

    // One stale blob sha: 409, all or nothing, the branch does not move.
    let (st, _) = commit(
        &w,
        R,
        "main",
        json!([
            {"operation": "update", "path": "projects/a.yaml", "content": b64("a: 2\n"), "sha": "0000000000000000000000000000000000000000"},
            {"operation": "create", "path": "projects/c.yaml", "content": b64("c: 1\n")},
        ]),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(head_of(&w, "main").await, first_sha);

    let (st, second) = commit(
        &w,
        R,
        "main",
        json!([{"operation": "update", "path": "projects/a.yaml", "content": b64("a: 2\n"), "sha": blob_sha}]),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{second}");
    let second_sha = second["Sha"].as_str().unwrap().to_owned();

    let (st, page, _) = call(
        &w.app,
        "GET",
        &format!("{R}/Shako.Tree(Ref=@r,Recursive=true,Skip=0,Top=1)?@r='main'"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{page}");
    assert_eq!(page["Tree"].as_array().unwrap().len(), 1);
    assert_eq!(page["Truncated"], true);

    let (st, history, _) = call(
        &w.app,
        "GET",
        &format!("{R}/Shako.History(Ref=@r,Path=@p,Top=1)?@r='main'&@p='projects/a.yaml'"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{history}");
    assert_eq!(history["value"].as_array().unwrap().len(), 1);
    assert_eq!(history["value"][0]["Sha"], second_sha);
    assert_eq!(history["value"][0]["Author"], "Alice Steward");

    let (st, one, _) = call(
        &w.app,
        "GET",
        &format!("{R}/Commits('{second_sha}')"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{one}");
    assert_eq!(one["Message"], "Portal change\n\nwith a body line");

    let (st, status, _) = call(
        &w.app,
        "GET",
        &format!("{R}/Shako.CommitStatus(Sha=@s)?@s='{second_sha}'"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{status}");
    assert_eq!(status["State"], "");
    assert_eq!(status["TotalCount"], 0);

    // Protecting main refuses the next contents write there.
    let (st, _, _) = call(
        &w.app,
        "POST",
        &format!("{R}/Shako.Protect"),
        Some("portal-token"),
        Some(json!({"Rule": {"rule_name": "main", "enable_push": false}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = commit(
        &w,
        R,
        "main",
        json!([{"operation": "create", "path": "projects/d.yaml", "content": b64("d: 1\n")}]),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "a protected branch is 403");
}

#[tokio::test]
async fn pull_requests_review_merge_and_native_webhooks() {
    let w = world();
    create_repo(&w, "helsinki").await;
    let (st, hook, _) = call(
        &w.app,
        "POST",
        &format!("{R}/Shako.AddWebhook"),
        Some("portal-token"),
        Some(json!({"Url": HOOK_URL, "Secret": HOOK_SECRET})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{hook}");
    assert!(
        !hook.to_string().contains(HOOK_SECRET),
        "the secret never comes back"
    );

    let (st, _, _) = call(
        &w.app,
        "POST",
        &format!("{R}/Shako.CreateBranch"),
        Some("portal-token"),
        Some(json!({"NewBranchName": "portal/change-1", "OldBranchName": "main"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = commit(
        &w,
        R,
        "portal/change-1",
        json!([{"operation": "create", "path": "projects/x.yaml", "content": b64("x: 1\n")}]),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // The push was delivered in shako's native shape, signed with X-Shako-Signature.
    {
        let got = w.delivered.lock();
        let (headers, body) = got.last().expect("a push delivery");
        let header = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        };
        assert_eq!(header("X-Shako-Event").as_deref(), Some("push"));
        assert!(header("X-Gitea-Signature").is_none());
        let sig = header("X-Shako-Signature").expect("signed");
        assert!(verify_gitea_signature(HOOK_SECRET, body, &sig));
        let event: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(event["event"], "push");
        assert_eq!(event["ref"], "refs/heads/portal/change-1");
        assert_eq!(event["repository"]["owner"], "joinedcontext");
        assert_eq!(event["repository"]["name"], "helsinki");
        assert_eq!(event["repository"]["defaultBranch"], "main");
    }

    let open = json!({"Head": "portal/change-1", "Base": "main", "Title": "Add x", "Body": "why"});
    let (st, pr, _) = call(
        &w.app,
        "POST",
        &format!("{R}/Shako.OpenPullRequest"),
        Some("portal-token"),
        Some(open.clone()),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pr}");
    assert_eq!(pr["Number"], 1);
    assert_eq!(pr["Mergeable"], true);
    assert_eq!(
        pr["HtmlUrl"],
        "https://city.example/git/joinedcontext/helsinki/pulls/1"
    );
    let (st, _, _) = call(
        &w.app,
        "POST",
        &format!("{R}/Shako.OpenPullRequest"),
        Some("portal-token"),
        Some(open),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "one open pull request per head and base"
    );

    let (st, list, _) = call(
        &w.app,
        "GET",
        &format!(
            "{R}/PullRequests?$filter=State%20eq%20'open'&$orderby=UpdatedAt%20desc&$top=10&$skip=0"
        ),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{list}");
    assert_eq!(list["value"].as_array().unwrap().len(), 1);
    assert_eq!(list["value"][0]["Title"], "Add x");

    let (st, files, _) = call(
        &w.app,
        "GET",
        &format!("{R}/PullRequests(1)/Files"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{files}");
    assert_eq!(files["value"][0]["Filename"], "projects/x.yaml");
    assert_eq!(files["value"][0]["Status"], "added");

    let (st, _, _) = call(
        &w.app,
        "POST",
        &format!("{R}/PullRequests(1)/Shako.Review"),
        Some("portal-token"),
        Some(json!({"Event": "COMMENT", "Body": "looks right"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, reviews, _) = call(
        &w.app,
        "GET",
        &format!("{R}/PullRequests(1)/Reviews"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(reviews["value"][0]["Body"], "looks right");

    let (st, _, _) = call(
        &w.app,
        "POST",
        &format!("{R}/PullRequests(1)/Shako.Merge"),
        Some("portal-token"),
        Some(json!({"Style": "merge", "HeadCommitId": "0000000000000000000000000000000000000000"})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "a moved head is 409");

    let (st, merged, _) = call(
        &w.app,
        "POST",
        &format!("{R}/PullRequests(1)/Shako.Merge"),
        Some("portal-token"),
        Some(json!({"Style": "merge", "Message": "Merge the change"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{merged}");
    let merge_sha = merged["Sha"].as_str().unwrap().to_owned();
    assert_eq!(head_of(&w, "main").await, merge_sha);

    let (st, pr, _) = call(
        &w.app,
        "GET",
        &format!("{R}/PullRequests(1)"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(pr["Merged"], true);
    assert_eq!(pr["MergeCommitSha"], merge_sha);

    let got = w.delivered.lock();
    let (_, body) = got.last().expect("the merge delivery");
    let event: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(event["event"], "pull_request");
    assert_eq!(event["action"], "closed");
    assert_eq!(event["pullRequest"]["merged"], true);
    assert_eq!(event["pullRequest"]["mergeCommitSha"], merge_sha);
}

#[tokio::test]
async fn mirrors_packages_and_tags() {
    let w = world();
    create_repo(&w, "helsinki").await;
    let credential = "ghp_odatamirrorcredential987654";
    let (st, mirror, raw) = call(
        &w.app,
        "POST",
        &format!("{R}/Shako.AddPushMirror"),
        Some("portal-token"),
        Some(json!({
            "RemoteAddress": "https://github.com/joinedcontext-apps/helsinki.git",
            "RemoteUsername": "x-access-token",
            "Credential": credential,
            "SyncOnCommit": true,
            "Interval": "8h0m0s",
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{mirror}");
    assert!(!String::from_utf8_lossy(&raw).contains(credential));
    let name = mirror["Name"].as_str().unwrap().to_owned();
    let (st, list, raw) = call(
        &w.app,
        "GET",
        &format!("{R}/PushMirrors"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{list}");
    assert_eq!(list["value"][0]["Name"], name);
    assert!(!String::from_utf8_lossy(&raw).contains(credential));
    let (st, _, _) = call(
        &w.app,
        "POST",
        &format!("{R}/Shako.RemovePushMirror"),
        Some("portal-token"),
        Some(json!({"Name": name})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let publish = json!({"Owner": "joinedcontext", "Package": "bikes-ui", "Version": "1.0.0", "File": "app.tar", "ContentBase64": b64("bundle bytes")});
    let (st, file, _) = call(
        &w.app,
        "POST",
        "/odata/PublishPackage",
        Some("portal-token"),
        Some(publish.clone()),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{file}");
    let (st, _, _) = call(
        &w.app,
        "POST",
        "/odata/PublishPackage",
        Some("portal-token"),
        Some(publish),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "a package file never changes");
    let (st, _, bytes) = call(
        &w.app,
        "GET",
        "/odata/PackageFiles(Owner='joinedcontext',Name='bikes-ui',Version='1.0.0',File='app.tar')/$value",
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(bytes, b"bundle bytes");

    let (st, tags, _) = call(
        &w.app,
        "GET",
        &format!("{R}/Tags?$top=50&$skip=0"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{tags}");
    assert!(tags["value"].as_array().unwrap().is_empty());
}

/// Every write and read the Portal makes, without a token: 401.
#[tokio::test]
async fn no_token_is_401_on_every_portal_call() {
    let w = world();
    create_repo(&w, "helsinki").await;
    for (method, uri) in [
        ("GET", R.to_owned()),
        ("DELETE", R.to_owned()),
        ("POST", "/odata/CreateRepository".to_owned()),
        ("POST", "/odata/ImportRepository".to_owned()),
        ("POST", format!("{R}/Shako.CommitFiles")),
        ("POST", format!("{R}/Shako.OpenPullRequest")),
        ("POST", format!("{R}/PullRequests(1)/Shako.Merge")),
        ("POST", format!("{R}/Shako.AddWebhook")),
        ("POST", format!("{R}/Shako.AddPushMirror")),
        ("POST", "/odata/PublishPackage".to_owned()),
        ("GET", format!("{R}/Branches('main')")),
        ("GET", format!("{R}/PullRequests(1)/Files")),
        ("GET", format!("{R}/Shako.Tree(Ref=@r)?@r='main'")),
        (
            "GET",
            "/odata/PackageFiles(Owner='joinedcontext',Name='p',Version='1',File='f')/$value"
                .to_owned(),
        ),
    ] {
        let (st, _, _) = call(&w.app, method, &uri, None, Some(json!({}))).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

/// Another organization's repository is 404 on every call, with the body a missing one gets.
#[tokio::test]
async fn another_organizations_repository_is_404_like_a_missing_one() {
    let w = world();
    create_repo(&w, "helsinki").await;
    let (st, _) = commit(
        &w,
        R,
        "main",
        json!([{"operation": "create", "path": "a.yaml", "content": b64("a\n")}]),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let missing = "/odata/Repositories(Owner='joinedcontext',Name='no-such-repo')";
    for (method, tail, body) in [
        ("GET", String::new(), None),
        ("DELETE", String::new(), None),
        ("GET", "/Branches('main')".to_owned(), None),
        (
            "GET",
            "/Shako.File(Path=@p,Ref=@r)?@p='a.yaml'&@r='main'".to_owned(),
            None,
        ),
        ("GET", "/PullRequests(1)/Files".to_owned(), None),
        (
            "POST",
            "/Shako.CommitFiles".to_owned(),
            Some(
                json!({"Message": "m", "Files": [{"operation": "create", "path": "b.yaml", "content": b64("b")}]}),
            ),
        ),
        ("POST", "/Shako.Archive".to_owned(), None),
        (
            "POST",
            "/Shako.AddWebhook".to_owned(),
            Some(json!({"Url": HOOK_URL, "Secret": "s"})),
        ),
    ] {
        let (st_cross, body_cross, _) = call(
            &w.app,
            method,
            &format!("{R}{tail}"),
            Some("org-b-token"),
            body.clone(),
        )
        .await;
        let (st_missing, body_missing, _) = call(
            &w.app,
            method,
            &format!("{missing}{tail}"),
            Some("portal-token"),
            body,
        )
        .await;
        assert_eq!(
            st_cross,
            StatusCode::NOT_FOUND,
            "{method} {tail}: {body_cross}"
        );
        assert_eq!(st_missing, StatusCode::NOT_FOUND, "{method} {tail}");
        assert_eq!(body_cross, body_missing, "{method} {tail}");
    }
    assert_eq!(head_of(&w, "main").await.len(), 40, "org B moved nothing");
}

/// A reader of a visible repository is 403 on a write, never 404.
#[tokio::test]
async fn a_reader_is_403_on_writes() {
    let w = world();
    create_repo(&w, "helsinki").await;
    let (st, _, _) = call(&w.app, "GET", R, Some("bob-token"), None).await;
    assert_eq!(st, StatusCode::OK);
    for (tail, body) in [
        (
            "/Shako.CommitFiles",
            json!({"Message": "m", "Files": [{"operation": "create", "path": "b.yaml", "content": b64("b")}]}),
        ),
        ("/Shako.CreateBranch", json!({"NewBranchName": "x"})),
        ("/Shako.AddWebhook", json!({"Url": HOOK_URL, "Secret": "s"})),
        ("/Shako.Protect", json!({"Rule": {"rule_name": "main"}})),
    ] {
        let (st, body, _) = call(
            &w.app,
            "POST",
            &format!("{R}{tail}"),
            Some("bob-token"),
            Some(body),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{tail}: {body}");
    }
}

/// The files and reviews of a pull request take the same read check as the pull request: a
/// person who reads one repository of the organization does not read another's by its id.
#[tokio::test]
async fn pull_request_parts_check_the_repository_read() {
    let w = world();
    create_repo(&w, "helsinki").await;
    create_repo(&w, "public-one").await;
    let (st, _, _) = call(
        &w.app,
        "POST",
        &format!("{R}/Shako.CreateBranch"),
        Some("portal-token"),
        Some(json!({"NewBranchName": "f"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = commit(
        &w,
        R,
        "f",
        json!([{"operation": "create", "path": "hidden.yaml", "content": b64("h\n")}]),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, pr, _) = call(
        &w.app,
        "POST",
        &format!("{R}/Shako.OpenPullRequest"),
        Some("portal-token"),
        Some(json!({"Head": "f", "Base": "main", "Title": "t"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pr}");
    let key = pr["Id"].as_str().unwrap();
    for part in ["Files", "Reviews"] {
        let (st, body, _) = call(
            &w.app,
            "GET",
            &format!("/odata/PullRequests('{key}')/{part}"),
            Some("carol-token"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{part}: {body}");
    }
}

/// Redelivering a hook goes only to a hook of the named repository.
#[tokio::test]
async fn a_hook_of_another_repository_is_not_redelivered() {
    let w = world();
    create_repo(&w, "helsinki").await;
    create_repo(&w, "other").await;
    let (st, hook, _) = call(
        &w.app,
        "POST",
        "/odata/Repositories(Owner='joinedcontext',Name='other')/Shako.AddWebhook",
        Some("portal-token"),
        Some(json!({"Url": HOOK_URL, "Secret": HOOK_SECRET})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{hook}");
    let id = hook["Id"].as_str().unwrap();
    let before = w.delivered.lock().len();
    let (st, _, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/repos/joinedcontext/helsinki/hooks/{id}/tests"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(w.delivered.lock().len(), before, "nothing was sent");
    let (st, _, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/repos/joinedcontext/other/hooks/{id}/tests"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(w.delivered.lock().len(), before + 1);
}

/// A mirror password never overwrites a secret already stored under the reference it names.
#[tokio::test]
async fn a_mirror_password_does_not_overwrite_a_named_secret() {
    let w = world();
    create_repo(&w, "helsinki").await;
    let (st, _, _) = call(
        &w.app,
        "POST",
        "/api/v1/repos/joinedcontext/helsinki/push_mirrors",
        Some("portal-token"),
        Some(json!({
            "remote_address": "https://github.com/joinedcontext-apps/helsinki.git",
            "remote_password": "attacker-chosen",
            "secret_ref": "portal-webhook-secret",
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(
        w.state
            .secret_store
            .get_secret(OrgId::from_u128(10), "portal-webhook-secret")
            .as_deref(),
        Some("wh-secret-key-123")
    );
}
