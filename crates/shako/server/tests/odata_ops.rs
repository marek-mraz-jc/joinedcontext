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
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, Principal,
    RoleBinding, ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_server::{ShakoState, router};
use shako_store_memory::MemoryStore;
use std::sync::Arc;
use tower::ServiceExt;

const HOOK_URL: &str = "https://portal.city.example/api/v1/webhooks/shako";
/// A hook that answers after 200 ms, so two dispatches overlap.
const SLOW_HOOK_URL: &str = "https://portal.city.example/api/v1/webhooks/slow";
const HOOK_SECRET: &str = "odata-hook-secret-value";
const R: &str = "/odata/Repositories(Owner='joinedcontext',Name='helsinki')";

/// One delivery the outbound guard caught: its headers and body.
type Delivered = Arc<parking_lot::Mutex<Vec<(Vec<(String, String)>, Vec<u8>)>>>;

struct World {
    _tmp: tempfile::TempDir,
    state: ShakoState,
    store: Arc<MemoryStore>,
    app: Router,
    delivered: Delivered,
}

async fn world() -> World {
    let tmp = tempfile::tempdir().expect("tempdir");
    let org_a = OrgId::from_u128(10);
    let org_b = OrgId::from_u128(20);
    let binding = |org_id, principal: &str, actions: Vec<Action>, repo: Option<&str>| RoleBinding {
        org_id,
        principal: Principal::User(principal.into()),
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
    let person = |login: &str, org| Subject::authenticated(login, login, vec![org]);
    let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
        .with_static_tokens(vec![
            StaticTokenBinding::new("portal-token", person("jc-portal", org_a)),
            StaticTokenBinding::new("bob-token", person("reader-bob", org_a)),
            StaticTokenBinding::new("carol-token", person("scoped-carol", org_a)),
            StaticTokenBinding::new("org-b-token", person("admin-b", org_b)),
            StaticTokenBinding::new(
                "renamed-token",
                Subject::authenticated("jc-portal", "renamed", vec![org_a]),
            ),
            StaticTokenBinding::new(
                "namesake-token",
                Subject::authenticated("u-namesake", "jc-portal", vec![org_a]),
            ),
        ]);
    let delivered: Delivered = Arc::default();
    let sink = delivered.clone();
    let guard = OutboundGuard::new(OutboundPolicy {
        allowed_hosts: Some(vec!["github.com".into(), "portal.city.example".into()]),
        allow_loopback_for_tests: false,
        mock_handler: Some(Arc::new(move |_method, url, headers, body| {
            if url == SLOW_HOOK_URL {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            if url == HOOK_URL || url == SLOW_HOOK_URL {
                sink.lock().push((headers.to_vec(), body.to_vec()));
                return Some((202, Vec::new()));
            }
            None
        })),
        ..OutboundPolicy::default()
    });
    let store = Arc::new(MemoryStore::new());
    let state = ShakoState::new(
        InstanceId::from_u128(1),
        tmp.path().to_path_buf(),
        store.clone(),
        authz,
        verifier,
        guard,
    );
    let kept = state
        .secret_store
        .put_secret(org_a, "portal-webhook-secret", "wh-secret-key-123");
    kept.await.expect("the secret is kept");
    let mount = Mount::new("shako", "https", "city.example", "/git").expect("mount");
    let app = router(state.clone(), mount);
    World {
        _tmp: tmp,
        state,
        store,
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
    let w = world().await;
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

/// An owner name is a label inside one Organization: creating or importing under a name another
/// Organization holds answers as a free name does, and each Organization reads only its own.
#[tokio::test]
async fn a_name_another_organization_holds_answers_as_a_free_name() {
    let w = world().await;
    create_repo(&w, "helsinki").await;
    let org_b = "org-b-token";
    let source = json!({"Owner": "bbsk", "Name": "source"});
    let (st, body, _) = call(
        &w.app,
        "POST",
        "/odata/CreateRepository",
        Some(org_b),
        Some(source),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (st, _, _) = call(&w.app, "GET", R, Some(org_b), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "theirs, before a twin exists");
    let stolen = json!({"SourceOwner": "joinedcontext", "SourceName": "helsinki", "RepoOwner": "mine", "RepoName": "copy"});
    let (st, _, _) = call(
        &w.app,
        "POST",
        "/odata/ImportRepository",
        Some(org_b),
        Some(stolen),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "theirs is no source to import");

    for op in ["CreateRepository", "ImportRepository"] {
        let mut answers = Vec::new();
        // Held by the other Organization, then free.
        for owner in ["joinedcontext", "nobody"] {
            let name = if op == "CreateRepository" {
                "helsinki"
            } else {
                "copy"
            };
            let body = json!({
                "Owner": owner, "Name": name,
                "SourceOwner": "bbsk", "SourceName": "source", "RepoOwner": owner, "RepoName": name,
            });
            let (st, body, _) = call(
                &w.app,
                "POST",
                &format!("/odata/{op}"),
                Some(org_b),
                Some(body),
            )
            .await;
            let fields: Option<Vec<String>> = body.as_object().map(|o| o.keys().cloned().collect());
            answers.push((st, fields));
        }
        assert_eq!(answers[0], answers[1], "{op}: held elsewhere, free");
        assert_eq!(answers[0].0, StatusCode::OK, "{op}");
    }

    let (st, ours, _) = call(&w.app, "GET", R, Some("portal-token"), None).await;
    assert_eq!(st, StatusCode::OK, "{ours}");
    let (st, theirs, _) = call(&w.app, "GET", R, Some(org_b), None).await;
    assert_eq!(st, StatusCode::OK, "{theirs}");
    assert_ne!(
        ours["Id"], theirs["Id"],
        "two repositories, one per Organization"
    );
    let (st, _, _) = call(&w.app, "DELETE", R, Some(org_b), None).await;
    assert!(st.is_success(), "{st}");
    let (st, kept, _) = call(&w.app, "GET", R, Some("portal-token"), None).await;
    assert_eq!(
        (st, &kept["Id"]),
        (StatusCode::OK, &ours["Id"]),
        "a twin's delete leaves it"
    );
}

#[tokio::test]
async fn contents_history_tree_and_a_stale_sha_that_moves_nothing() {
    let w = world().await;
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
    let w = world().await;
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
    // A merged pull request keeps the base it landed on.
    let (st, body, _) = call(
        &w.app,
        "PATCH",
        "/api/v1/repos/joinedcontext/helsinki/pulls/1",
        Some("portal-token"),
        Some(json!({"base": "release"})),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

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
    let w = world().await;
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
    // A reader of the repository sees no mirror in the organization-wide set either.
    let (st, list, _) = call(&w.app, "GET", "/odata/PushMirrors", Some("bob-token"), None).await;
    assert_eq!(st, StatusCode::OK, "{list}");
    assert_eq!(list["value"], json!([]), "{list}");
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

/// Every write operation of the registry, through the door, leaves exactly one audit row that
/// commits with it (S6-20), carrying the registry's `audit_kind`, the actor's subject id and ids,
/// never a secret. A new write operation fails this test until it is in the table.
#[tokio::test]
async fn every_write_operation_leaves_one_audit_row() {
    use shako_core::Store as _;
    let w = world().await;
    let copy_r = "/odata/Repositories(Owner='joinedcontext',Name='helsinki-copy')";
    let compat = "/api/v1/repos/joinedcontext/helsinki";
    let file =
        |path: &str| json!([{"operation": "create", "path": path, "content": b64("x: 1\n")}]);
    let commit_body = |branch: &str, path: &str| json!({"Branch": branch, "Message": "change", "Files": file(path)});
    let mut steps: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "CreateRepository",
            "POST",
            "/odata/CreateRepository".into(),
            Some(json!({"Owner": "joinedcontext", "Name": "helsinki"})),
        ),
        (
            "ImportRepository",
            "POST",
            "/odata/ImportRepository".into(),
            Some(
                json!({"SourceOwner": "joinedcontext", "SourceName": "helsinki", "RepoOwner": "joinedcontext", "RepoName": "helsinki-copy"}),
            ),
        ),
        (
            "Archive",
            "POST",
            format!("{copy_r}/Shako.Archive"),
            Some(json!({})),
        ),
        ("Delete", "DELETE", copy_r.into(), None),
        (
            "AddWebhook",
            "POST",
            format!("{R}/Shako.AddWebhook"),
            Some(json!({"Url": HOOK_URL, "Secret": HOOK_SECRET})),
        ),
        (
            "CreateBranch",
            "POST",
            format!("{R}/Shako.CreateBranch"),
            Some(json!({"NewBranchName": "feature", "OldBranchName": "main"})),
        ),
        (
            "CommitFiles",
            "POST",
            format!("{R}/Shako.CommitFiles"),
            Some(commit_body("feature", "a.yaml")),
        ),
        (
            "OpenPullRequest",
            "POST",
            format!("{R}/Shako.OpenPullRequest"),
            Some(json!({"Head": "feature", "Base": "main", "Title": "Add a", "Body": "why"})),
        ),
        (
            "Review",
            "POST",
            format!("{R}/PullRequests(1)/Shako.Review"),
            Some(json!({"Event": "COMMENT", "Body": "looks right"})),
        ),
        (
            "CommitFiles",
            "POST",
            format!("{R}/Shako.CommitFiles"),
            Some(commit_body("main", "b.yaml")),
        ),
        (
            "UpdateBranch",
            "POST",
            format!("{R}/PullRequests(1)/Shako.UpdateBranch"),
            Some(json!({})),
        ),
        (
            "Merge",
            "POST",
            format!("{R}/PullRequests(1)/Shako.Merge"),
            Some(json!({"Style": "merge"})),
        ),
        (
            "CreateBranch",
            "POST",
            format!("{R}/Shako.CreateBranch"),
            Some(json!({"NewBranchName": "second", "OldBranchName": "main"})),
        ),
        (
            "CommitFiles",
            "POST",
            format!("{R}/Shako.CommitFiles"),
            Some(commit_body("second", "c.yaml")),
        ),
        (
            "OpenPullRequest",
            "POST",
            format!("{R}/Shako.OpenPullRequest"),
            Some(json!({"Head": "second", "Base": "main", "Title": "Add c", "Body": "why"})),
        ),
        (
            "PatchPullRequest",
            "PATCH",
            format!("{compat}/pulls/2"),
            Some(json!({"title": "Add c, renamed"})),
        ),
        (
            "Close",
            "POST",
            format!("{R}/PullRequests(2)/Shako.Close"),
            Some(json!({})),
        ),
        (
            "DeleteBranch",
            "POST",
            format!("{R}/Shako.DeleteBranch"),
            Some(json!({"Branch": "second"})),
        ),
        (
            "Protect",
            "POST",
            format!("{R}/Shako.Protect"),
            Some(json!({"Rule": {"rule_name": "release"}})),
        ),
        (
            "Unprotect",
            "POST",
            format!("{R}/Shako.Unprotect"),
            Some(json!({"Pattern": "release"})),
        ),
        (
            "AddPushMirror",
            "POST",
            format!("{R}/Shako.AddPushMirror"),
            Some(
                json!({"RemoteAddress": "https://github.com/joinedcontext-apps/helsinki.git", "Credential": "ghp_auditmirrorcredential12345", "SyncOnCommit": false}),
            ),
        ),
        (
            "SyncPushMirrors",
            "POST",
            format!("{R}/Shako.SyncPushMirrors"),
            Some(json!({})),
        ),
        (
            "PublishPackage",
            "POST",
            "/odata/PublishPackage".into(),
            Some(
                json!({"Owner": "joinedcontext", "Package": "bikes-ui", "Version": "1.0.0", "File": "app.tar", "ContentBase64": b64("bundle bytes")}),
            ),
        ),
        (
            "PatchRepository",
            "PATCH",
            compat.into(),
            Some(json!({"description": "City bikes"})),
        ),
    ];
    let rows = || {
        w.store
            .list_audit(InstanceId::from_u128(1), OrgId::from_u128(10))
    };
    let mut seen = Vec::new();
    let mut i = 0;
    while i < steps.len() {
        let (operation, method, uri, body) = steps[i].clone();
        let before = rows().await.expect("rows").len();
        let (st, answer, _) = call(&w.app, method, &uri, Some("portal-token"), body).await;
        assert!(st.is_success(), "{operation}: {st} {answer}");
        let after = rows().await.expect("rows");
        assert_eq!(after.len(), before + 1, "{operation} leaves one row");
        let row = after.last().expect("row");
        let kind = w
            .state
            .registry
            .find(operation)
            .filter(|op| op.name == operation)
            .map_or_else(
                || match operation {
                    "PatchRepository" => "REPO_UPDATE",
                    "PatchPullRequest" => "PR_UPDATE",
                    _ => "STATUS_SET",
                },
                |op| op.audit_kind,
            );
        assert_eq!(
            (row.operation.as_str(), row.kind.as_str()),
            (operation, kind)
        );
        assert_eq!(row.actor_id, "jc-portal");
        assert_eq!(row.decision, "allow");
        let text = serde_json::to_string(row).expect("row json");
        for never in [
            HOOK_SECRET,
            "ghp_auditmirrorcredential12345",
            "bundle bytes",
            "@",
        ] {
            assert!(
                !text.contains(never),
                "{operation}: the row holds {never}: {text}"
            );
        }
        // The steps that need an id the previous answer gave.
        match operation {
            "AddWebhook" => {
                let id = answer["Id"].as_str().expect("hook id").to_owned();
                steps.push((
                    "RedeliverWebhook",
                    "POST",
                    format!("/odata/Webhooks('{id}')/Shako.RedeliverWebhook"),
                    Some(json!({})),
                ));
            }
            "AddPushMirror" => {
                let name = answer["Name"].as_str().expect("mirror name").to_owned();
                steps.push((
                    "RemovePushMirror",
                    "POST",
                    format!("{R}/Shako.RemovePushMirror"),
                    Some(json!({"Name": name})),
                ));
            }
            "Merge" => {
                let sha = answer["Sha"].as_str().expect("merge sha").to_owned();
                steps.push((
                    "SetCommitStatus",
                    "POST",
                    format!("{compat}/statuses/{sha}"),
                    Some(json!({"state": "success", "context": "ci"})),
                ));
            }
            _ => {}
        }
        seen.push(operation);
        i += 1;
    }
    let missing: Vec<&str> = w
        .state
        .registry
        .operations()
        .iter()
        .filter(|op| op.rest_method != "GET")
        .map(|op| op.name)
        .filter(|name| !seen.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "write operations without an audit row step: {missing:?}"
    );
}

/// `$expand` on a set without navigation properties is a 400, never a silently smaller answer.
#[tokio::test]
async fn an_expand_on_a_set_that_expands_nothing_is_400() {
    let w = world().await;
    create_repo(&w, "helsinki").await;
    let (st, body, _) = call(
        &w.app,
        "GET",
        "/odata/Repositories?$expand=Branches",
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    let (st, body, _) = call(
        &w.app,
        "GET",
        &format!("{R}/PullRequests?$expand=Files,Reviews"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

/// Every answer carries one fixed content security policy: no page holds an inline style, so
/// there is no nonce to draw per request.
#[tokio::test]
async fn every_answer_carries_the_fixed_content_security_policy() {
    let w = world().await;
    let resp = w
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/odata/Repositories")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csp = resp.headers()["content-security-policy"].to_str().unwrap();
    assert!(
        csp.starts_with("default-src 'none'; style-src 'self'; "),
        "{csp}"
    );
    assert!(!csp.contains("nonce"), "{csp}");
}

/// Every write and read the Portal makes, without a token: 401.
#[tokio::test]
async fn no_token_is_401_on_every_portal_call() {
    let w = world().await;
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
    let w = world().await;
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
async fn a_rename_onto_a_taken_name_is_409_and_a_new_default_branch_moves_head() {
    let w = world().await;
    create_repo(&w, "helsinki").await;
    create_repo(&w, "other").await;
    let patch = |body: Value| {
        call(
            &w.app,
            "PATCH",
            "/api/v1/repos/joinedcontext/helsinki",
            Some("portal-token"),
            Some(body),
        )
    };
    let (st, body, _) = patch(json!({"name": "other"})).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    let (st, body, _) = patch(json!({"default_branch": "release"})).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (_, repo, _) = call(&w.app, "GET", R, Some("portal-token"), None).await;
    let id: shako_core::RepoId = repo["Id"].as_str().unwrap().parse().unwrap();
    let disk =
        shako_core::repo_disk_path(w.state.repo_root(), InstanceId::from_u128(1), id).unwrap();
    let head = std::fs::read_to_string(disk.join("HEAD")).unwrap();
    assert_eq!(head.trim(), "ref: refs/heads/release");
}

#[tokio::test]
async fn api_commits_count_against_the_quota_and_an_import_keeps_to_it() {
    let w = world().await;
    let (inst, org) = (InstanceId::from_u128(1), OrgId::from_u128(10));
    create_repo(&w, "helsinki").await;
    let usage = || async { w.state.store().get_quota_usage(inst, org).await.unwrap() };
    let size = || async {
        let (_, repo, _) = call(&w.app, "GET", R, Some("portal-token"), None).await;
        repo["SizeBytes"].as_u64().unwrap()
    };
    assert_eq!(usage().await.git_bytes, size().await);
    let big: String = (0..40_000u32)
        .map(|i| format!("{:x}", i.wrapping_mul(2_654_435_761)))
        .collect();
    let (st, body) = commit(
        &w,
        R,
        "main",
        json!([{"operation": "create", "path": "data.txt", "content": b64(&big)}]),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let grown = size().await;
    assert_eq!(usage().await.git_bytes, grown, "the commit's bytes count");

    let mut limits = shako_core::InstanceRecord::default_for(inst, org, "localhost");
    limits.max_repos = 1;
    w.state.store().upsert_instance(limits).await.unwrap();
    let (st, body, _) = call(
        &w.app,
        "POST",
        "/odata/ImportRepository",
        Some("portal-token"),
        Some(json!({"SourceOwner": "joinedcontext", "SourceName": "helsinki", "RepoOwner": "joinedcontext", "RepoName": "helsinki-copy"})),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "an import past max_repos: {body}"
    );
    assert_eq!(usage().await.repo_count, 1);
}

#[tokio::test]
async fn a_navigated_collection_keeps_to_its_repository_whatever_the_filter() {
    let w = world().await;
    create_repo(&w, "helsinki").await;
    create_repo(&w, "other").await;
    let (st, body, _) = call(
        &w.app,
        "POST",
        "/odata/Repositories(Owner='joinedcontext',Name='other')/Shako.Protect",
        Some("portal-token"),
        Some(json!({"Rule": {"rule_name": "main"}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let filter = "Pattern%20eq%20'main')%20or%20(Pattern%20eq%20'main'";
    let (st, body, _) = call(
        &w.app,
        "GET",
        &format!("{R}/ProtectionRules?$filter={filter}"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["value"], json!([]), "{body}");
}

#[tokio::test]
async fn a_reader_is_403_on_writes() {
    let w = world().await;
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
    let w = world().await;
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

/// An account is its subject id: renamed it is the same author, and its namesake is nobody.
#[tokio::test]
async fn an_account_is_its_subject_id() {
    let w = world().await;
    create_repo(&w, "helsinki").await;
    let (st, me, _) = call(&w.app, "GET", "/odata/Me()", Some("renamed-token"), None).await;
    assert_eq!((st, &me["Login"]), (StatusCode::OK, &json!("jc-portal")));
    let (st, _, _) = call(&w.app, "GET", R, Some("namesake-token"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let files = json!([{"operation": "create", "path": "f.yaml", "content": b64("f\n")}]);
    let post = |path: &str, token: &'static str, body: Value| {
        let (app, path) = (w.app.clone(), format!("{R}/{path}"));
        async move { call(&app, "POST", &path, Some(token), Some(body)).await.0 }
    };
    let branch = json!({"NewBranchName": "f"});
    assert_eq!(
        post("Shako.CreateBranch", "portal-token", branch).await,
        StatusCode::OK
    );
    assert_eq!(commit(&w, R, "f", files).await.0, StatusCode::OK);
    let open = json!({"Head": "f", "Base": "main", "Title": "t"});
    assert_eq!(
        post("Shako.OpenPullRequest", "portal-token", open).await,
        StatusCode::OK
    );
    let rule = json!({"Rule": {"rule_name": "main", "required_approvals": 1}});
    assert_eq!(
        post("Shako.Protect", "portal-token", rule).await,
        StatusCode::OK
    );
    let approve = json!({"Event": "APPROVED"});
    assert_eq!(
        post("PullRequests(1)/Shako.Review", "renamed-token", approve).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the author approving under another display name"
    );
}

/// T-7508: a store that cannot say what protects a branch protects it: no merge and no
/// self-approval runs on an empty answer in its place.
#[tokio::test]
async fn an_unreadable_store_never_reads_as_no_rules() {
    let w = world().await;
    create_repo(&w, "helsinki").await;
    let branch = json!({"NewBranchName": "f"});
    let files = json!([{"operation": "create", "path": "f.yaml", "content": b64("f\n")}]);
    let post = |path: String, body: Value| {
        let app = w.app.clone();
        async move {
            call(&app, "POST", &path, Some("portal-token"), Some(body))
                .await
                .0
        }
    };
    assert_eq!(
        post(format!("{R}/Shako.CreateBranch"), branch).await,
        StatusCode::OK
    );
    assert_eq!(commit(&w, R, "f", files).await.0, StatusCode::OK);
    let open = json!({"Head": "f", "Base": "main", "Title": "t"});
    assert_eq!(
        post(format!("{R}/Shako.OpenPullRequest"), open).await,
        StatusCode::OK
    );
    let main = head_of(&w, "main").await;

    let merge = format!("{R}/PullRequests(1)/Shako.Merge");
    for table in ["reviews", "commit_statuses", "protection_rules"] {
        w.store.simulate_read_failure(table, true);
        let refused = post(merge.clone(), json!({"Style": "merge"})).await;
        assert_eq!(
            refused,
            StatusCode::INTERNAL_SERVER_ERROR,
            "merge without {table}"
        );
        w.store.simulate_read_failure(table, false);
    }
    w.store.simulate_read_failure("protection_rules", true);
    let approve = format!("{R}/PullRequests(1)/Shako.Review");
    let refused = post(approve, json!({"Event": "APPROVED"})).await;
    assert_eq!(
        refused,
        StatusCode::INTERNAL_SERVER_ERROR,
        "self-approval without the rules"
    );
    let (head, _, _) = call(
        &w.app,
        "GET",
        &format!("{R}/Branches('main')"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(
        head,
        StatusCode::INTERNAL_SERVER_ERROR,
        "protected or not is unknown"
    );
    w.store.simulate_read_failure("protection_rules", false);
    assert_eq!(head_of(&w, "main").await, main);
    assert_eq!(post(merge, json!({"Style": "merge"})).await, StatusCode::OK);
}

/// T-7508: an unreadable quota is refused, never replaced by the wider defaults.
#[tokio::test]
async fn an_unreadable_quota_never_reads_as_the_defaults() {
    let w = world().await;
    w.store.simulate_read_failure("instances", true);
    let (st, body, _) = call(
        &w.app,
        "POST",
        "/odata/CreateRepository",
        Some("portal-token"),
        Some(json!({"Owner": "joinedcontext", "Name": "helsinki", "DefaultBranch": "main"})),
    )
    .await;
    assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
}

/// Redelivering a hook goes only to a hook of the named repository.
#[tokio::test]
async fn a_hook_of_another_repository_is_not_redelivered() {
    let w = world().await;
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

/// Writes of one Organization at once deliver each event once, even when a hook is slow.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_commits_deliver_each_event_once() {
    let w = world().await;
    create_repo(&w, "helsinki").await;
    let (st, hook, _) = call(
        &w.app,
        "POST",
        &format!("{R}/Shako.AddWebhook"),
        Some("portal-token"),
        Some(json!({"Url": SLOW_HOOK_URL, "Secret": HOOK_SECRET})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{hook}");
    let before = w.delivered.lock().len();
    // Each write on a runtime of its own: the slow hook blocks only its own thread.
    let writes: Vec<_> = (0..3)
        .map(|i| {
            let app = w.app.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async move {
                let files = json!([{"operation": "create", "path": format!("f{i}.txt"), "content": b64("x\n")}]);
                call(
                    &app,
                    "POST",
                    &format!("{R}/Shako.CommitFiles"),
                    Some("portal-token"),
                    Some(json!({"Branch": "main", "Message": "one file", "Files": files})),
                )
                .await
                .0
                })
            })
        })
        .collect();
    for write in writes {
        assert_eq!(write.join().unwrap(), StatusCode::OK);
    }
    assert_eq!(
        w.delivered.lock().len() - before,
        3,
        "one delivery per commit"
    );
}

/// A hook's target address can carry its own credential, so hooks and their deliveries are
/// Admin reads like the repository's mirrors: a reader sees none, the administrator sees each.
#[tokio::test]
async fn a_reader_sees_no_hook_or_delivery_of_the_repository() {
    let w = world().await;
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
    let id = hook["Id"].as_str().unwrap();
    let (st, _, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/repos/joinedcontext/helsinki/hooks/{id}/tests"),
        Some("portal-token"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    for path in [
        "/odata/Webhooks".to_owned(),
        "/odata/Deliveries".to_owned(),
        format!("{R}/Webhooks"),
    ] {
        let (st, admin, _) = call(&w.app, "GET", &path, Some("portal-token"), None).await;
        assert_eq!(st, StatusCode::OK, "{path}: {admin}");
        assert_eq!(admin["value"].as_array().map(Vec::len), Some(1), "{path}");
        let (st, reader, _) = call(&w.app, "GET", &path, Some("bob-token"), None).await;
        assert_eq!(st, StatusCode::OK, "{path}: {reader}");
        assert_eq!(reader["value"], json!([]), "{path}");
    }
}

/// A commit through the API passes the secret scan a push passes: a literal secret in a manifest
/// is refused with the rule and path, never the value, and nothing lands on the branch.
#[tokio::test]
async fn a_literal_secret_in_an_api_commit_is_refused() {
    let w = world().await;
    create_repo(&w, "helsinki").await;
    let before = head_of(&w, "main").await;
    let literal = "s3cr3t".repeat(2);
    let manifest = format!("apiVersion: v1\nkind: Connector\nspec:\n  password: {literal}\n");
    let (st, body) = commit(
        &w,
        R,
        "main",
        json!([{"operation": "create", "path": "connectors/a.yaml", "content": b64(&manifest)}]),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let said = body.to_string();
    assert!(
        said.contains("connectors/a.yaml") && !said.contains(&literal),
        "{said}"
    );
    assert_eq!(head_of(&w, "main").await, before);
}

/// A secret reference is a key name of the secret store: a caller who pastes a credential there
/// gets 422 naming the field, and nothing reaches the store (whose log writes the reference in
/// clear); the answer never repeats the value.
#[tokio::test]
async fn a_credential_pasted_as_a_secret_reference_is_refused() {
    let w = world().await;
    create_repo(&w, "helsinki").await;
    let pasted = &format!("{}_abcdefghijklmnopqrstuvwxyz0123456789", "ghp");
    for (uri, body, field) in [
        (
            "/api/v1/repos/joinedcontext/helsinki/hooks",
            json!({ "url": HOOK_URL, "secret_ref": pasted }),
            "secretRef",
        ),
        (
            "/api/v1/repos/joinedcontext/helsinki/push_mirrors",
            json!({
                "remote_address": "https://github.com/joinedcontext-apps/helsinki.git",
                "secret_ref": pasted,
            }),
            "secretRef",
        ),
    ] {
        let (st, answer, raw) = call(&w.app, "POST", uri, Some("portal-token"), Some(body)).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{uri}: {answer}");
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains(field), "{uri}: {text}");
        assert!(!text.contains(pasted), "{uri}: the value is echoed");
    }
    let org = OrgId::from_u128(10);
    let instance = w.state.default_instance_id;
    assert!(
        w.state
            .store()
            .list_webhooks(instance, org, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        w.state
            .store()
            .list_push_mirrors(instance, org, None)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A mirror password never overwrites a secret already stored under the reference it names.
#[tokio::test]
async fn a_mirror_password_does_not_overwrite_a_named_secret() {
    let w = world().await;
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
            .await
            .expect("the store answers"),
        Some("wh-secret-key-123".to_owned())
    );
}
