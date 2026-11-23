#![forbid(unsafe_code)]
//! Every compat and native REST route refuses before it reads or writes: no token is 401, a token
//! of another Organization and an unknown repository are 404 alike, and a reader cannot write.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use shako_api::{ShakoState, compat_and_native_rest_router};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, Principal,
    RoleBinding, ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_store_memory::MemoryStore;
use std::sync::Arc;
use tower::ServiceExt;

const ADMIN: &str = "admin-token";
const READER: &str = "reader-token";
const OTHER: &str = "other-org-token";

fn world(root: &std::path::Path) -> (ShakoState, Router) {
    let (org, other) = (OrgId::from_u128(10), OrgId::from_u128(20));
    let binding = |org_id, user: &str, actions| RoleBinding {
        org_id,
        principal: Principal::User(user.into()),
        actions,
        project_id: None,
        repo: None,
    };
    let authz = Arc::new(LocalAuthorizer::new(vec![
        binding(
            org,
            "admin",
            vec![Action::Admin, Action::Write, Action::Approve],
        ),
        binding(org, "reader", vec![Action::Read]),
        binding(other, "stranger", vec![Action::Admin, Action::Write]),
    ]));
    let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
        .with_static_tokens(vec![
            StaticTokenBinding::new(ADMIN, Subject::authenticated("admin", "admin", vec![org])),
            StaticTokenBinding::new(
                READER,
                Subject::authenticated("reader", "reader", vec![org]),
            ),
            StaticTokenBinding::new(
                OTHER,
                Subject::authenticated("stranger", "stranger", vec![other]),
            ),
        ]);
    let state = ShakoState::new(
        InstanceId::from_u128(1),
        root.to_path_buf(),
        Arc::new(MemoryStore::new()),
        authz,
        verifier,
        OutboundGuard::new(OutboundPolicy::default()),
    );
    let app = compat_and_native_rest_router().with_state(state.clone());
    (state, app)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("authorization", format!("token {t}"));
    }
    let req = match body {
        Some(v) => req
            .header("content-type", "application/json")
            .body(Body::from(v.to_string())),
        None => req.body(Body::empty()),
    }
    .expect("request");
    let resp = app.clone().oneshot(req).await.expect("answer");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Every route that names a repository, with a body its extractor accepts, so the handler's own
/// checks answer. `{r}` is the repository's path prefix (compat or native).
fn repo_routes() -> Vec<(&'static str, String, Option<Value>)> {
    let author = json!({ "name": "A", "email": "a@city.example" });
    let mut routes: Vec<(&'static str, String, Option<Value>)> = Vec::new();
    for r in [
        "/api/v1/repos/joinedcontext/nope",
        "/api/v1/projects/joinedcontext/repositories/nope",
    ] {
        routes.extend([
            ("GET", r.to_owned(), None),
            ("PATCH", r.to_owned(), Some(json!({ "description": "d" }))),
            ("DELETE", r.to_owned(), None),
            ("POST", format!("{r}/branches"), Some(json!({ "new_branch_name": "b" }))),
            ("GET", format!("{r}/branches/main"), None),
            ("DELETE", format!("{r}/branches/b"), None),
            ("POST", format!("{r}/contents"), Some(json!({ "files": [{ "operation": "create", "path": "a.txt", "content": "YQ==" }], "message": "m", "author": author }))),
            ("GET", format!("{r}/contents/a.txt"), None),
            ("PUT", format!("{r}/contents/a.txt"), Some(json!({ "content": "YQ==", "message": "m" }))),
            ("DELETE", format!("{r}/contents/a.txt"), Some(json!({ "message": "m", "sha": "0".repeat(40) }))),
            ("GET", format!("{r}/pulls"), None),
            ("POST", format!("{r}/pulls"), Some(json!({ "title": "t", "head": "b", "base": "main" }))),
            ("GET", format!("{r}/pulls/1"), None),
            ("PATCH", format!("{r}/pulls/1"), Some(json!({ "title": "t" }))),
            ("GET", format!("{r}/pulls/1/reviews"), None),
            ("POST", format!("{r}/pulls/1/reviews"), Some(json!({ "event": "APPROVED" }))),
            ("POST", format!("{r}/pulls/1/merge"), Some(json!({ "Do": "merge" }))),
            ("POST", format!("{r}/pulls/1/update"), None),
        ]);
    }
    let r = "/api/v1/repos/joinedcontext/nope";
    routes.extend([
        ("GET", format!("{r}/push_mirrors"), None),
        ("POST", format!("{r}/push_mirrors"), Some(json!({ "remote_address": "https://github.com/a/b.git" }))),
        ("DELETE", format!("{r}/push_mirrors/m1"), None),
        ("POST", format!("{r}/push_mirrors-sync"), None),
        ("GET", format!("{r}/branch_protections"), None),
        ("POST", format!("{r}/branch_protections"), Some(json!({ "rule_name": "main" }))),
        ("GET", format!("{r}/branch_protections/main"), None),
        ("PATCH", format!("{r}/branch_protections/main"), Some(json!({}))),
        ("DELETE", format!("{r}/branch_protections/main"), None),
        ("GET", format!("{r}/git/trees/main"), None),
        ("GET", format!("{r}/commits/main/status"), None),
        ("POST", format!("{r}/statuses/{}", "0".repeat(40)), Some(json!({ "state": "success" }))),
        ("GET", format!("{r}/commits"), None),
        ("GET", format!("{r}/git/commits/{}", "0".repeat(40)), None),
        ("GET", format!("{r}/archive/main.zip"), None),
        ("GET", "/joinedcontext/nope/archive/main.tar.gz".to_owned(), None),
        ("GET", format!("{r}/tags"), None),
        ("GET", format!("{r}/pulls/1/files"), None),
        ("POST", format!("{r}/hooks"), Some(json!({ "type": "gitea", "config": { "url": "https://portal.city.example/hook", "content_type": "json" }, "events": ["push"], "active": true }))),
        ("POST", format!("{r}/hooks/0190a1b2-0000-7000-8000-000000000001/tests"), None),
    ]);
    routes
}

#[tokio::test]
async fn every_repository_route_refuses_a_caller_it_does_not_serve() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (_state, app) = world(tmp.path());
    for (method, uri, body) in repo_routes() {
        let (status, err) = send(&app, method, &uri, None, body.as_ref()).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} without a token: {err}"
        );
        let (status, err) = send(&app, method, &uri, Some("not-a-token"), body.as_ref()).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} with a bad token: {err}"
        );
        for token in [ADMIN, OTHER] {
            let (status, err) = send(&app, method, &uri, Some(token), body.as_ref()).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "{method} {uri} as {token}: {err}"
            );
            assert!(
                err["message"].as_str().is_some_and(|m| !m.is_empty()),
                "{method} {uri}: {err}"
            );
        }
    }
}

#[tokio::test]
async fn organization_routes_refuse_without_a_token() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (_state, app) = world(tmp.path());
    let routes: Vec<(&str, &str, Option<Value>)> = vec![
        ("GET", "/api/v1/version", None),
        ("GET", "/api/v1/user", None),
        (
            "GET",
            "/api/v1/users/admin/orgs/joinedcontext/permissions",
            None,
        ),
        ("GET", "/api/v1/orgs/joinedcontext/teams", None),
        (
            "POST",
            "/api/v1/orgs/joinedcontext/teams",
            Some(json!({ "name": "t" })),
        ),
        ("DELETE", "/api/v1/teams/1", None),
        ("GET", "/api/v1/teams/1/repos", None),
        ("PUT", "/api/v1/teams/1/repos/joinedcontext/nope", None),
        ("DELETE", "/api/v1/teams/1/repos/joinedcontext/nope", None),
        (
            "POST",
            "/api/v1/orgs/joinedcontext/repos",
            Some(json!({ "name": "r" })),
        ),
        (
            "POST",
            "/api/v1/projects/joinedcontext/repositories",
            Some(json!({ "name": "r" })),
        ),
        (
            "POST",
            "/api/v1/repos/migrate",
            Some(
                json!({ "clone_addr": "https://github.com/a/b.git", "repo_owner": "joinedcontext", "repo_name": "b" }),
            ),
        ),
        ("GET", "/api/v1/repos/joinedcontext/nope/actions/runs", None),
        (
            "POST",
            "/api/v1/repos/joinedcontext/nope/actions/workflows/ci.yml/dispatches",
            None,
        ),
        ("GET", "/api/v1/orgs/joinedcontext/actions/jobs", None),
        (
            "POST",
            "/api/v1/repos/joinedcontext/nope/actions/runners/registration-token",
            None,
        ),
        (
            "GET",
            "/api/v1/repos/joinedcontext/nope/actions/artifacts",
            None,
        ),
        (
            "GET",
            "/api/v1/repos/joinedcontext/nope/actions/artifacts/1/zip",
            None,
        ),
    ];
    for (method, uri, body) in &routes {
        let (status, err) = send(&app, method, uri, None, body.as_ref()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}: {err}");
    }
    let (status, body) = send(&app, "GET", "/api/healthz", None, None).await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "status": "ok" })));
    let (status, body) = send(&app, "GET", "/api/v1/version", Some(READER), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["version"], json!("1.27.3+shako"));
    let (status, me) = send(&app, "GET", "/api/v1/user", Some(READER), None).await;
    assert_eq!(
        (status, me["login"].clone(), me["email"].clone()),
        (StatusCode::OK, json!("reader"), json!(""))
    );
    for (token, rights) in [(READER, [false, false, true]), (ADMIN, [true, true, true])] {
        let (status, got) = send(
            &app,
            "GET",
            "/api/v1/users/x/orgs/joinedcontext/permissions",
            Some(token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            got,
            json!({ "is_owner": false, "is_admin": rights[0], "can_write": rights[1], "can_read": rights[2] }),
            "{token}"
        );
    }
    for uri in ["/api/v1/orgs/joinedcontext/teams", "/api/v1/teams/1/repos"] {
        assert_eq!(
            send(&app, "GET", uri, Some(READER), None).await,
            (StatusCode::OK, json!([])),
            "{uri}"
        );
    }
    // Teams and collaborators are the Organization's role bindings: every write is 501.
    for (method, uri, body) in [
        (
            "POST",
            "/api/v1/orgs/joinedcontext/teams",
            Some(json!({ "name": "t" })),
        ),
        ("DELETE", "/api/v1/teams/1", None),
        ("PUT", "/api/v1/teams/1/repos/joinedcontext/nope", None),
        ("DELETE", "/api/v1/teams/1/repos/joinedcontext/nope", None),
        (
            "PUT",
            "/api/v1/repos/joinedcontext/nope/collaborators/bob",
            None,
        ),
    ] {
        let (status, err) = send(&app, method, uri, None, body.as_ref()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        let (status, err2) = send(&app, method, uri, Some(ADMIN), body.as_ref()).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{method} {uri}: {err}");
        assert!(
            err2["message"]
                .as_str()
                .is_some_and(|m| m.contains("role bindings")),
            "{err2}"
        );
    }
    for uri in [
        "/api/v1/repos/joinedcontext/nope/actions/runs",
        "/api/v1/orgs/joinedcontext/actions/jobs",
    ] {
        let (status, body) = send(&app, "GET", uri, Some(READER), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|m| m.contains("actions")),
            "{body}"
        );
    }
}
