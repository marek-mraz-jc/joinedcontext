//! The in-memory forge every target asks: one Organization, a reader and an admin token, one
//! repository with a branch, a commit, a pull request and a package, seeded once per process.

use std::sync::{Arc, LazyLock};

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use serde_json::json;
use shako_api::{ShakoState, compat_and_native_rest_router, odata_router};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, Principal,
    RoleBinding, ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_store_memory::MemoryStore;
use tower::ServiceExt;

pub const READER: &str = "reader-token";
const ADMIN: &str = "admin-token";

pub static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
});

static ROOT: LazyLock<tempfile::TempDir> =
    LazyLock::new(|| tempfile::tempdir().expect("a repository root"));

pub static APP: LazyLock<Router> = LazyLock::new(|| {
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
    ]));
    let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
        .with_static_tokens(vec![
            StaticTokenBinding::new(ADMIN, Subject::authenticated("admin", "admin", vec![org])),
            StaticTokenBinding::new(
                READER,
                Subject::authenticated("reader", "reader", vec![org]),
            ),
        ]);
    // No host is allowed: a fuzzed mirror or hook address never leaves the process.
    let guard = OutboundGuard::new(OutboundPolicy {
        allowed_hosts: Some(Vec::new()),
        ..OutboundPolicy::default()
    });
    let state = ShakoState::new(
        InstanceId::from_u128(1),
        ROOT.path().to_path_buf(),
        Arc::new(MemoryStore::new()),
        authz,
        verifier,
        guard,
    );
    let app = compat_and_native_rest_router()
        .merge(odata_router())
        .with_state(state);
    RT.block_on(async {
        let seed = json!({ "name": "site", "auto_init": true, "default_branch": "main" });
        let (status, _) = send(
            &app,
            "POST",
            "/api/v1/orgs/joinedcontext/repos",
            ADMIN,
            Some(seed.to_string().into_bytes()),
        )
        .await;
        assert!(status < 300, "seeding the repository: {status}");
        let branch = json!({ "new_branch_name": "feature" });
        let (status, _) = send(
            &app,
            "POST",
            "/api/v1/repos/joinedcontext/site/branches",
            ADMIN,
            Some(branch.to_string().into_bytes()),
        )
        .await;
        assert!(status < 300, "seeding a branch: {status}");
    });
    app
});

/// One request; its status and body length. `None` for a URI the client could not form.
pub async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Vec<u8>>,
) -> (u16, usize) {
    let Ok(req) = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("token {token}"))
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, Body::from))
    else {
        return (0, 0);
    };
    let Ok(resp) = app.clone().oneshot(req).await;
    let status = resp.status().as_u16();
    let len = axum::body::to_bytes(resp.into_body(), 1 << 24)
        .await
        .map_or(0, |b| b.len());
    (status, len)
}

/// Splits fuzz input into a method selector, a path and a body at the first two newlines.
pub fn parts(data: &[u8]) -> Option<(u8, String, Vec<u8>)> {
    let (&which, rest) = data.split_first()?;
    let text = String::from_utf8_lossy(rest);
    let (path, body) = text.split_once('\n').unwrap_or((&text, ""));
    Some((which, path.to_owned(), body.as_bytes().to_vec()))
}
