#![forbid(unsafe_code)]
//! A browser on shako's pages: shako keeps no sessions, so a request without `Authorization` is
//! sent to the Portal's page for the same repository, computed from the path alone. A signed-out
//! caller gets the same answer for a repository that exists and one that does not; a caller with
//! a bearer (API, MCP, CLI) reads the page as before.

use axum::Router;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use shako_api::ShakoState;
use shako_api::service::{NewRepository, create_repository};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, RequestCtx, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_store_memory::MemoryStore;
use shako_web::{PortalBase, web_router};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "portal-token-0123456789";
const PATHS: [&str; 4] = [
    "/",
    "/joinedcontext/helsinki",
    "/joinedcontext/helsinki/blob/main/README.md",
    "/joinedcontext/helsinki/pulls",
];

/// A shako with the repository `joinedcontext/helsinki`, or with none.
async fn shako(tmp: &tempfile::TempDir, with_repo: bool) -> Router {
    let org = OrgId::from_u128(10);
    let subject = Subject::authenticated("jc-portal", "jc-portal", vec![org], vec![]);
    let state = ShakoState::new(
        InstanceId::from_u128(1),
        org,
        tmp.path().to_path_buf(),
        Arc::new(MemoryStore::new()),
        Arc::new(LocalAuthorizer::new(vec![RoleBinding {
            org_id: org,
            principal: "jc-portal".into(),
            actions: vec![Action::Admin, Action::Write, Action::Read],
            project_id: None,
            repo: None,
        }])),
        TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
            .with_static_tokens(vec![StaticTokenBinding::new(TOKEN, subject.clone())]),
        OutboundGuard::default(),
    );
    state.register_owner_org("joinedcontext", org);
    if with_repo {
        let ctx = RequestCtx::new(InstanceId::from_u128(1), org, subject);
        let repo = NewRepository {
            owner: "joinedcontext".into(),
            name: "helsinki".into(),
            description: String::new(),
            private: true,
            auto_init: true,
            default_branch: "main".into(),
        };
        create_repository(&state, &ctx, repo).await.expect("repo");
    }
    web_router().with_state(state)
}

/// Status, `Location` and body of a GET.
async fn get(app: &Router, path: &str, token: Option<&str>) -> (StatusCode, String, String) {
    let mut req = Request::builder().uri(path);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("token {t}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let location = resp
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        location,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

#[tokio::test]
async fn a_signed_out_browser_cannot_tell_an_existing_repository_from_a_missing_one() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (with, without) = (shako(&a, true).await, shako(&b, false).await);
    for path in PATHS {
        let seen = get(&with, path, None).await;
        assert_eq!(seen.0, StatusCode::FOUND, "{path}");
        assert_eq!(seen, get(&without, path, None).await, "{path}");
    }
    assert_eq!(
        get(&with, "/joinedcontext/helsinki/blob/main/README.md", None)
            .await
            .1,
        "/projects/helsinki/repositories/helsinki"
    );
    assert_eq!(get(&with, "/", None).await.1, "/");
}

#[tokio::test]
async fn a_bearer_reads_the_pages_as_before() {
    let tmp = tempfile::tempdir().unwrap();
    let app = shako(&tmp, true).await;
    let (status, location, body) = get(&app, "/joinedcontext/helsinki", Some(TOKEN)).await;
    assert_eq!((status, location.as_str()), (StatusCode::OK, ""), "{body}");
    assert!(body.contains("helsinki"), "{body}");
    let (status, ..) = get(&app, "/joinedcontext/missing", Some(TOKEN)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, ..) = get(&app, "/joinedcontext/helsinki", Some("not-a-token")).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a wrong bearer is no browser"
    );
}

#[tokio::test]
async fn the_redirect_lands_on_the_portal_base_and_never_off_it() {
    let tmp = tempfile::tempdir().unwrap();
    let app = shako(&tmp, true)
        .await
        .layer(Extension(PortalBase("https://city.example/".into())));
    for (path, target) in [
        (
            "/joinedcontext/helsinki/commits/main",
            "https://city.example/projects/helsinki/repositories/helsinki",
        ),
        ("//evil.example/x", "https://city.example/"),
        ("/joinedcontext/%2F%2Fevil.example", "https://city.example/"),
        (
            "/joinedcontext/a%2F..%2Fevil/pulls",
            "https://city.example/",
        ),
    ] {
        let (status, location, _) = get(&app, path, None).await;
        assert_eq!(status, StatusCode::FOUND, "{path}");
        assert_eq!(location, target, "{path}");
    }
    // The Gitea sign-in shim keeps its own guarded target.
    let (_, location, _) = get(&app, "/user/oauth2/keycloak?redirect_to=//evil", None).await;
    assert_eq!(location, "/");
}
