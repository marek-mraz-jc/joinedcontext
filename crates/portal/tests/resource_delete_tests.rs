use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::api::mutate::branch_name;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::Operation;
use joinedcontext_portal::change::{Change, ChangePhase, Lane};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ProblemDetails;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_CSRF_TOKEN: &str = "test-csrf-token-12345";

fn make_session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@banskabystrica.sk".into()),
            name: Some("Demo Steward".into()),
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let raw = value.to_str().expect("cookie header");
        let pair = raw.split(';').next().unwrap_or_default();
        parts.push(pair.to_string());
    }
    parts.join("; ")
}

fn session_and_csrf_cookies(config: &Config) -> String {
    let session = make_session_cookie(config);
    format!("{session}; {CSRF_COOKIE}={TEST_CSRF_TOKEN}")
}

#[tokio::test]
async fn delete_returns_202_with_change_and_commits_to_gitea() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-space-123",
            "content": "YXBpVmVyc2lvbjogeW91"
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;

    mount_tree_and_commit(&server, &["projects/ovzdusie/spaces/mobility/space.yaml"]).await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 55,
            "html_url": "https://gitea.example.sk/pulls/55",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "mobility".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": false }),
        status: None,
    });

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let change: Change = serde_json::from_slice(&body_bytes).expect("deserialize Change");
    assert_eq!(change.api_version, API_VERSION);
    assert_eq!(change.kind, "Change");
    assert_eq!(change.metadata.name, "chg-00000037");
    assert_eq!(change.metadata.namespace, "ovzdusie");
    assert_eq!(change.status.lane, Lane::Red);
    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert_eq!(change.status.plan.create, 0);
    assert_eq!(change.status.plan.update, 0);
    assert_eq!(change.status.plan.delete, 1);
    assert_eq!(
        change.status.merge_request.as_deref(),
        Some("https://gitea.example.sk/pulls/55")
    );

    let requests = server.received_requests().await.expect("received requests");

    let branch_req = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/repos/test-owner/test-repo/branches"
        })
        .expect("branch creation request");
    let branch_body: serde_json::Value =
        serde_json::from_slice(&branch_req.body).expect("branch request body");
    assert!(branch_body["new_branch_name"]
        .as_str()
        .expect("new branch name")
        .starts_with("portal/delete-contextspace-mobility-"));
    assert_eq!(branch_body["old_branch_name"], "main");

    let delete_req = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/repos/test-owner/test-repo/contents"
        })
        .expect("the commit that removes the files");
    let delete_body: serde_json::Value =
        serde_json::from_slice(&delete_req.body).expect("commit request body");
    assert_eq!(delete_body["author"]["name"], "Demo Steward");
    assert_eq!(
        delete_body["author"]["email"],
        "demo.steward@banskabystrica.sk"
    );
    assert_eq!(delete_body["committer"]["name"], "Demo Steward");
    assert_eq!(
        delete_body["committer"]["email"],
        "demo.steward@banskabystrica.sk"
    );
    let files = delete_body["files"].as_array().expect("the files removed");
    assert_eq!(files.len(), 1, "the manifest alone: {files:?}");
    assert_eq!(files[0]["operation"], "delete");
    assert_eq!(
        files[0]["path"],
        "projects/ovzdusie/spaces/mobility/space.yaml"
    );
    assert_eq!(files[0]["sha"], "sha-space-123");
    assert_eq!(delete_body["message"], "delete ContextSpace mobility");

    let pulls_req = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/repos/test-owner/test-repo/pulls"
        })
        .expect("pull request creation request");
    let pulls_body: serde_json::Value =
        serde_json::from_slice(&pulls_req.body).expect("pull request body");
    assert_eq!(pulls_body["title"], "delete ContextSpace mobility");
    assert_eq!(pulls_body["base"], "main");
    assert!(pulls_body["head"]
        .as_str()
        .expect("head branch")
        .starts_with("portal/delete-contextspace-mobility-"));
}

#[tokio::test]
async fn delete_missing_name_returns_404() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/nonexistent")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 404);
    assert_eq!(
        problem.r#type,
        "https://joinedcontext.com/errors/resource-not-found"
    );
    assert_eq!(
        problem.detail.as_deref(),
        Some("resource 'nonexistent' not found in project 'ovzdusie'")
    );
}

#[tokio::test]
async fn delete_unknown_plural_returns_404() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/unknownplurals/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 404);
    assert_eq!(
        problem.r#type,
        "https://joinedcontext.com/errors/resource-not-found"
    );
    assert_eq!(
        problem.detail.as_deref(),
        Some("resource 'mobility' not found in project 'ovzdusie'")
    );
}

/// AG-77, R20: a blocked deletion names the references of the caller's own project, which the
/// caller reads anyway, and would only count those of other projects.
#[tokio::test]
async fn delete_blocked_by_dependents_returns_409_naming_this_projects_references() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "mobility".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": false }),
        status: None,
    });

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Endpoint".to_string(),
        metadata: ObjectMeta {
            name: "live-traffic".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({
            "spaceRef": {
                "kind": "ContextSpace",
                "name": "mobility"
            }
        }),
        status: None,
    });

    let app = server::app(state.clone());

    let response1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response1.status(), StatusCode::CONFLICT);
    assert_eq!(
        response1.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let bytes1 = response1
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem1: ProblemDetails = serde_json::from_slice(&bytes1).expect("ProblemDetails");
    assert_eq!(problem1.status, 409);
    assert_eq!(problem1.r#type, "https://joinedcontext.com/errors/conflict");
    assert_eq!(
        problem1.detail.as_deref(),
        Some("1 dependent resource blocks deletion: Endpoint live-traffic")
    );

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Pipeline".to_string(),
        metadata: ObjectMeta {
            name: "traffic-stream".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({
            "space": {
                "kind": "ContextSpace",
                "name": "mobility"
            }
        }),
        status: None,
    });

    let response2 = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response2.status(), StatusCode::CONFLICT);
    let bytes2 = response2
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem2: ProblemDetails = serde_json::from_slice(&bytes2).expect("ProblemDetails");
    assert_eq!(problem2.status, 409);
    assert_eq!(
        problem2.detail.as_deref(),
        Some(
            "2 dependent resources block deletion: Endpoint live-traffic, Pipeline traffic-stream"
        )
    );

    let requests = server.received_requests().await.expect("received requests");
    assert!(requests.is_empty());
}

#[tokio::test]
async fn delete_without_forge_returns_503() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "mobility".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": false }),
        status: None,
    });

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 503);
    assert_eq!(
        problem.r#type,
        "https://joinedcontext.com/errors/service-unavailable"
    );
    assert_eq!(
        problem.detail.as_deref(),
        Some("git forge is not configured")
    );
}

#[tokio::test]
async fn delete_without_csrf_header_returns_403() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "mobility".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": false }),
        status: None,
    });

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 403);
    assert_eq!(problem.r#type, "https://joinedcontext.com/errors/forbidden");
}

fn space_state(client: GiteaClient) -> (Config, AppState) {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "mobility".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": false }),
        status: None,
    });
    (config, state)
}

/// The tree the removal reads to find the files the resource owns beside its manifest, and
/// the one commit that removes them (T-0900).
async fn mount_tree_and_commit(server: &MockServer, tree: &[&str]) {
    let entries: Vec<serde_json::Value> = tree
        .iter()
        .map(|path| json!({ "path": path, "type": "blob" }))
        .collect();
    Mock::given(method("GET"))
        .and(path_regex(
            r"^/api/v1/repos/test-owner/test-repo/git/trees/.*$",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "tree": entries, "truncated": false })),
        )
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/contents"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "commit": { "sha": "commit-sha-deleted" }
        })))
        .mount(server)
        .await;
}

async fn mount_repo_and_file(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-sha",
            "content": "YXBpVmVyc2lvbjogam9pbmVkY29udGV4dC5jb20vdjFhbHBoYTEK",
            "encoding": "base64"
        })))
        .mount(server)
        .await;
}

async fn delete_mobility(config: &Config, state: AppState) -> axum::response::Response {
    server::app(state)
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
}

/// A branch an earlier attempt left behind is recreated from main, never reused: on dev the
/// stale branch had the file already deleted and the removal answered 404 (T-0886).
#[tokio::test]
async fn a_stale_branch_is_recreated_from_main_before_the_removal_is_written() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    mount_repo_and_file(&server).await;
    let branch = branch_name("ovzdusie", "ContextSpace", "mobility", Operation::Delete);
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    // The first creation finds the stale branch; after it is dropped the second one succeeds.
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(409).set_body_string("branch already exists"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/branches/{branch}"
        )))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    mount_tree_and_commit(&server, &["projects/ovzdusie/spaces/mobility/space.yaml"]).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 56,
            "html_url": "https://gitea.example.sk/pulls/56",
            "state": "open",
            "merged": false
        })))
        .mount(&server)
        .await;

    let (config, state) = space_state(client);
    let response = delete_mobility(&config, state).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let requests = server.received_requests().await.expect("received requests");
    let names: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/branches"))
        .map(|r| {
            r.body_json::<serde_json::Value>().expect("branch body")["new_branch_name"]
                .as_str()
                .expect("name")
                .to_string()
        })
        .collect();
    assert_eq!(names[0], branch, "the deterministic name is tried first");
    assert!(
        names[1].starts_with(&format!("{branch}_")) && names[1] != branch,
        "the retry opens on a fresh name: {}",
        names[1]
    );
    let heads: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/pulls"))
        .map(|r| {
            r.body_json::<serde_json::Value>().expect("pull body")["head"]
                .as_str()
                .expect("head")
                .to_string()
        })
        .collect();
    assert_eq!(
        heads,
        vec![names[1].clone()],
        "the pull request opens on the fresh name"
    );
}

/// A removal already under review is decided first: the second one names it (T-0883).
#[tokio::test]
async fn a_second_removal_while_one_is_open_names_the_open_change() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    mount_repo_and_file(&server).await;
    let branch = branch_name("ovzdusie", "ContextSpace", "mobility", Operation::Delete);
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "number": 300,
            "html_url": "https://gitea.example.sk/pulls/300",
            "state": "open",
            "head": { "ref": branch },
            "base": { "ref": "main" },
            "merged": false
        }])))
        .mount(&server)
        .await;

    let (config, state) = space_state(client);
    let response = delete_mobility(&config, state).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&body).expect("problem");
    assert_eq!(
        problem.detail.as_deref(),
        Some("a change for ContextSpace 'mobility' is already open: chg-0000012c; approve or reject it first")
    );
    let requests = server.received_requests().await.expect("received requests");
    assert!(
        requests.iter().all(|r| r.method.as_str() == "GET"),
        "nothing was written"
    );
}

/// T-0900: a DataModel is a manifest plus its LinkML source and whatever was rendered from it.
/// On dev the manifest went and the source stayed, so the next model of that name would have
/// inherited a schema nobody wrote for it.
#[tokio::test]
async fn deleting_a_datamodel_removes_its_source_and_its_artefacts_in_one_change() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    for (file, sha) in [
        ("datamodels/bikes.yaml", "sha-manifest"),
        ("datamodels/bikes.linkml.yaml", "sha-source"),
        ("datamodels/bikes.schema.json", "sha-schema"),
        ("datamodels/shared.linkml.yaml", "sha-shared"),
    ] {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/{file}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": sha,
                "content": "YXBpVmVyc2lvbjogam9pbmVkY29udGV4dC5jb20vdjFhbHBoYTEK",
                "encoding": "base64"
            })))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    mount_tree_and_commit(
        &server,
        &[
            "projects/ovzdusie/spaces/mobility/datamodels/bikes.yaml",
            "projects/ovzdusie/spaces/mobility/datamodels/bikes.linkml.yaml",
            "projects/ovzdusie/spaces/mobility/datamodels/bikes.schema.json",
            // Another model's source, named after it and kept.
            "projects/ovzdusie/spaces/mobility/datamodels/shared.linkml.yaml",
            "projects/ovzdusie/spaces/mobility/space.yaml",
        ],
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 57,
            "html_url": "https://gitea.example.sk/pulls/57",
            "state": "open",
            "merged": false
        })))
        .mount(&server)
        .await;

    let (config, state) = space_state(client);
    let model = |name: &str, linkml: &str| ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "DataModel".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "contextSpaceRef": "mobility", "linkml": linkml, "version": "0.1.0" }),
        status: None,
    };
    state.mirror.upsert(model("bikes", "./bikes.linkml.yaml"));
    state.mirror.upsert(model("shared", "./shared.linkml.yaml"));

    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/datamodels/bikes")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let requests = server.received_requests().await.expect("received requests");
    let commit = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/repos/test-owner/test-repo/contents"
        })
        .expect("the commit that removes the files");
    let body: serde_json::Value =
        serde_json::from_slice(&commit.body).expect("commit request body");
    let removed: Vec<&str> = body["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| file["path"].as_str().expect("a path"))
        .collect();

    assert!(removed.contains(&"projects/ovzdusie/spaces/mobility/datamodels/bikes.yaml"));
    assert!(removed.contains(&"projects/ovzdusie/spaces/mobility/datamodels/bikes.linkml.yaml"));
    assert!(removed.contains(&"projects/ovzdusie/spaces/mobility/datamodels/bikes.schema.json"));
    assert!(
        !removed
            .iter()
            .any(|path| path.contains("shared") || path.ends_with("space.yaml")),
        "only the model's own files: {removed:?}"
    );
    assert!(body["files"]
        .as_array()
        .expect("files")
        .iter()
        .all(|file| file["operation"] == "delete"));
}

/// The other half of T-0900: a source another model still names is not this model's to remove.
#[tokio::test]
async fn a_source_another_manifest_still_names_survives_the_delete() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    for (file, sha) in [
        ("datamodels/bikes.yaml", "sha-manifest"),
        ("datamodels/bikes.linkml.yaml", "sha-source"),
    ] {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/{file}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": sha,
                "content": "YXBpVmVyc2lvbjogam9pbmVkY29udGV4dC5jb20vdjFhbHBoYTEK",
                "encoding": "base64"
            })))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    mount_tree_and_commit(
        &server,
        &[
            "projects/ovzdusie/spaces/mobility/datamodels/bikes.yaml",
            "projects/ovzdusie/spaces/mobility/datamodels/bikes.linkml.yaml",
        ],
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 58,
            "html_url": "https://gitea.example.sk/pulls/58",
            "state": "open",
            "merged": false
        })))
        .mount(&server)
        .await;

    let (config, state) = space_state(client);
    let model = |name: &str, linkml: &str| ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "DataModel".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "contextSpaceRef": "mobility", "linkml": linkml, "version": "0.1.0" }),
        status: None,
    };
    state.mirror.upsert(model("bikes", "./bikes.linkml.yaml"));
    // A second model that reads the first one's source: the file is not the first one's alone.
    state
        .mirror
        .upsert(model("cargo-bikes", "./bikes.linkml.yaml"));

    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/datamodels/bikes")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let requests = server.received_requests().await.expect("received requests");
    let commit = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/repos/test-owner/test-repo/contents"
        })
        .expect("the commit that removes the files");
    let body: serde_json::Value =
        serde_json::from_slice(&commit.body).expect("commit request body");
    let removed: Vec<&str> = body["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| file["path"].as_str().expect("a path"))
        .collect();
    assert_eq!(
        removed,
        vec!["projects/ovzdusie/spaces/mobility/datamodels/bikes.yaml"],
        "the manifest alone: another model reads that source"
    );
}

// ---- T-2517: `delete_with_identity`, a removal under the caller's own rights (MF-07, PF-50,
// CC-19, CC-76, R20) ----------------------------------------------------------------------------

mod under_identity {
    use super::*;
    use joinedcontext_portal::api::delete::{delete_with_identity, DeleteOutcome, Reference};
    use joinedcontext_portal::ops::workspaces::{Opening, Scope};
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    use serde_json::Value;

    const HERE: &str = "ovzdusie";
    const THERE: &str = "doprava";

    fn manifest(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: kind.into(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        }
    }

    fn who(name: &str, groups: &[&str]) -> Identity {
        Identity {
            client: None,
            subject: format!("sub-{name}"),
            username: name.into(),
            email: Some(format!("{name}@hel.fi")),
            name: None,
            roles: Vec::new(),
            groups: groups.iter().map(|g| (*g).to_owned()).collect(),
            second_factor: false,
        }
    }

    /// Removes anything in the project: the bootstrap group of `Config::for_tests`.
    fn steward() -> Identity {
        who("jana", &["portal-approver"])
    }

    /// Reads every kind in the project and deletes Pipelines only.
    fn reader() -> Identity {
        who("vera", &["readers"])
    }

    fn space_ref(namespace: Option<&str>) -> Value {
        let mut reference = json!({ "kind": "ContextSpace", "name": "mobility" });
        if let Some(namespace) = namespace {
            reference["namespace"] = json!(namespace);
        }
        json!({ "contextSpaceRef": reference })
    }

    /// The space `mobility` of `ovzdusie` and whatever `extra` holds, with the reader's binding.
    fn state_with(gitea: Option<&MockServer>, extra: Vec<ResourceEnvelope>) -> AppState {
        let mut state = AppState::new(Config::for_tests(), None);
        if let Some(server) = gitea {
            let client = GiteaClient::new(
                server.uri().parse().expect("url"),
                "test-owner",
                "test-repo",
                "t",
            )
            .expect("a forge client");
            state = state.with_gitea(Arc::new(client));
        }
        state.mirror.upsert(manifest(
            "ContextSpace",
            "mobility",
            HERE,
            json!({ "isSandbox": true }),
        ));
        state.mirror.upsert(manifest(
            "Role",
            "reads-all",
            ORG_NAMESPACE,
            json!({ "rules": [
                { "kinds": ["ContextSpace", "Endpoint", "Pipeline"], "verbs": ["read"] },
                { "kinds": ["Pipeline"], "verbs": ["delete"] },
            ] }),
        ));
        state.mirror.upsert(manifest(
            "RoleBinding",
            "readers",
            ORG_NAMESPACE,
            json!({ "role": "reads-all", "subjects": [{ "group": "readers" }], "scope": { "project": HERE } }),
        ));
        for envelope in extra {
            state.mirror.upsert(envelope);
        }
        state
    }

    async fn remove(
        state: &AppState,
        identity: &Identity,
        plural: &str,
        name: &str,
        dry_run: bool,
        workspace: Option<&str>,
    ) -> Result<DeleteOutcome, joinedcontext_portal::error::ApiError> {
        delete_with_identity(identity, state, HERE, plural, name, dry_run, workspace).await
    }

    /// The status a refusal answers with, and its sentence.
    fn answer(err: joinedcontext_portal::error::ApiError) -> (StatusCode, String) {
        use axum::response::IntoResponse;
        let said = err.to_string();
        (err.into_response().status(), said)
    }

    fn referenced(outcome: DeleteOutcome) -> (Vec<Reference>, usize) {
        match outcome {
            DeleteOutcome::Referenced { here, elsewhere } => (here, elsewhere),
            other => panic!("not refused by its references: {other:?}"),
        }
    }

    fn refs(list: &[(&str, &str)]) -> Vec<Reference> {
        list.iter()
            .map(|(kind, name)| Reference {
                kind: (*kind).into(),
                name: (*name).into(),
            })
            .collect()
    }

    /// T-2517, MF-07, R20: another project's reference blocks the removal and is counted; its
    /// kind, name and project never reach the caller.
    #[tokio::test]
    async fn a_foreign_project_reference_is_counted_never_named() {
        let state = state_with(
            None,
            vec![
                manifest("Endpoint", "their-feed", THERE, space_ref(Some(HERE))),
                manifest(
                    "Pipeline",
                    "their-sync",
                    THERE,
                    json!({ "steps": [space_ref(Some(HERE))] }),
                ),
                // Another project's own `mobility` is not this one.
                manifest("Endpoint", "own-space", THERE, space_ref(None)),
                manifest("Endpoint", "air", HERE, space_ref(None)),
            ],
        );
        for dry_run in [true, false] {
            let (here, elsewhere) = referenced(
                remove(&state, &steward(), "spaces", "mobility", dry_run, None)
                    .await
                    .expect("an outcome"),
            );
            assert_eq!(here, refs(&[("Endpoint", "air")]), "dry run: {dry_run}");
            assert_eq!(elsewhere, 2, "dry run: {dry_run}");
            let said = DeleteOutcome::conflict_message(&here, elsewhere);
            for secret in ["their-feed", "their-sync", THERE] {
                assert!(!said.contains(secret), "{said}");
            }
        }
    }

    /// T-2517, PF-50: reading a kind is not deleting it, and deleting one kind is not another.
    #[tokio::test]
    async fn a_caller_without_delete_on_the_kind_is_refused_even_with_read() {
        let state = state_with(None, vec![manifest("Pipeline", "sync", HERE, json!({}))]);
        for dry_run in [true, false] {
            let refused = remove(&state, &reader(), "spaces", "mobility", dry_run, None)
                .await
                .expect_err("no delete on ContextSpace");
            let (status, said) = answer(refused);
            assert_eq!(status, StatusCode::FORBIDDEN, "{said}");
        }
        // The same caller removes the kind it holds `delete` on, as far as a dry run goes.
        assert!(matches!(
            remove(&state, &reader(), "pipelines", "sync", true, None).await,
            Ok(DeleteOutcome::DryRun(_))
        ));
    }

    /// T-2517, R20: an unknown plural and an unknown name answer the same 404, before any
    /// permission is looked at, so a caller with no binding learns nothing more than one with.
    #[tokio::test]
    async fn an_unknown_plural_is_404_before_any_permission_check() {
        let state = state_with(None, Vec::new());
        let stranger = who("nobody", &[]);
        for identity in [&stranger, &steward()] {
            for (plural, name) in [
                ("widgets", "mobility"),
                ("spaces", "no-such-space"),
                ("", "mobility"),
            ] {
                let missing = remove(&state, identity, plural, name, true, None)
                    .await
                    .expect_err("missing");
                let (status, said) = answer(missing);
                assert_eq!(status, StatusCode::NOT_FOUND, "{plural}/{name}");
                assert_eq!(
                    said,
                    format!("not found: resource '{name}' not found in project '{HERE}'"),
                    "{plural}/{name}"
                );
            }
        }
    }

    /// T-2517: a resource that names itself (a space pointing at its own name) is not its own
    /// dependent; a same-named resource of another kind still is.
    #[tokio::test]
    async fn a_resource_referencing_itself_is_not_counted_as_a_dependent() {
        let state = state_with(
            None,
            vec![manifest(
                "ContextSpace",
                "mobility",
                HERE,
                json!({ "isSandbox": true, "parent": { "kind": "ContextSpace", "name": "mobility" } }),
            )],
        );
        assert!(matches!(
            remove(&state, &steward(), "spaces", "mobility", true, None).await,
            Ok(DeleteOutcome::DryRun(_))
        ));
        state
            .mirror
            .upsert(manifest("Endpoint", "mobility", HERE, space_ref(None)));
        let (here, elsewhere) = referenced(
            remove(&state, &steward(), "spaces", "mobility", true, None)
                .await
                .expect("an outcome"),
        );
        assert_eq!((here, elsewhere), (refs(&[("Endpoint", "mobility")]), 0));
    }

    /// T-2517, MF-07: every dependent of this project is named with its kind, sorted by kind and
    /// then name, whatever order the mirror holds them in.
    #[tokio::test]
    async fn two_dependents_of_different_kinds_are_both_named_and_sorted() {
        let state = state_with(
            None,
            vec![
                manifest(
                    "Pipeline",
                    "zeta",
                    HERE,
                    json!({ "target": space_ref(None) }),
                ),
                manifest("Endpoint", "beta", HERE, space_ref(None)),
                manifest(
                    "Pipeline",
                    "alpha",
                    HERE,
                    json!({ "sources": [space_ref(None)] }),
                ),
                manifest("Endpoint", "alpha", HERE, space_ref(None)),
            ],
        );
        let (here, elsewhere) = referenced(
            remove(&state, &steward(), "spaces", "mobility", true, None)
                .await
                .expect("an outcome"),
        );
        assert_eq!(
            here,
            refs(&[
                ("Endpoint", "alpha"),
                ("Endpoint", "beta"),
                ("Pipeline", "alpha"),
                ("Pipeline", "zeta")
            ])
        );
        assert_eq!(elsewhere, 0);
    }

    #[tokio::test]
    async fn a_dependent_in_the_same_project_but_different_kind_is_named_with_its_kind() {
        let state = state_with(
            None,
            vec![manifest("Pipeline", "mobility-sync", HERE, space_ref(None))],
        );
        let (here, _) = referenced(
            remove(&state, &steward(), "spaces", "mobility", true, None)
                .await
                .expect("an outcome"),
        );
        assert_eq!(here, refs(&[("Pipeline", "mobility-sync")]));
        let said = DeleteOutcome::conflict_message(&here, 0);
        assert!(
            said.contains("Pipeline") && said.contains("mobility-sync"),
            "{said}"
        );
    }

    /// T-2517, CC-19: nothing references it, so the removal plans in the red lane, deleting the
    /// one resource.
    #[tokio::test]
    async fn zero_dependents_proceeds_to_the_red_lane_change() {
        let state = state_with(None, Vec::new());
        match remove(&state, &steward(), "spaces", "mobility", true, None).await {
            Ok(DeleteOutcome::DryRun(result)) => {
                assert!(result.valid);
                assert_eq!(result.lane, Lane::Red);
                assert_eq!(
                    serde_json::to_value(&result.plan).expect("a plan")["summary"]["delete"],
                    1
                );
            }
            other => panic!("{other:?}"),
        }
    }

    /// T-2517: a dry run reaches the same verdict as the real removal and writes nothing: the
    /// references block both, and the forge is never asked.
    #[tokio::test]
    async fn dry_run_true_reports_the_same_referenced_outcome_without_proposing() {
        let forge = MockServer::start().await;
        let state = state_with(
            Some(&forge),
            vec![manifest("Endpoint", "air", HERE, space_ref(None))],
        );
        let dry = referenced(
            remove(&state, &steward(), "spaces", "mobility", true, None)
                .await
                .expect("dry"),
        );
        let real = referenced(
            remove(&state, &steward(), "spaces", "mobility", false, None)
                .await
                .expect("real"),
        );
        assert_eq!(dry, real);
        assert!(forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
        // Without references, the dry run is a plan and still no forge call.
        let free = state_with(Some(&forge), Vec::new());
        assert!(matches!(
            remove(&free, &steward(), "spaces", "mobility", true, None).await,
            Ok(DeleteOutcome::DryRun(_))
        ));
        assert!(forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
    }

    /// T-2517, CC-76, R20: a workspace nobody holds, or one of another project, is refused with
    /// one sentence before the forge is asked for any tree.
    #[tokio::test]
    async fn a_workspace_name_that_does_not_exist_is_refused_before_the_mirror_is_read() {
        let forge = MockServer::start().await;
        let state = state_with(Some(&forge), Vec::new());
        state
            .workspaces
            .create(Opening {
                name: "elsewhere",
                title: None,
                project: THERE,
                owner: "jana@hel.fi",
                base_revision: "base1",
                scope: Scope::Project {},
                ttl_hours: 2,
            })
            .await
            .expect("opened");
        for workspace in ["no-such-copy", "elsewhere"] {
            let refused = remove(
                &state,
                &steward(),
                "spaces",
                "mobility",
                true,
                Some(workspace),
            )
            .await
            .expect_err(workspace);
            let (status, said) = answer(refused);
            assert_eq!(status, StatusCode::NOT_FOUND, "{workspace}");
            assert_eq!(
                said,
                format!("not found: no workspace named '{workspace}' in project '{HERE}'")
            );
        }
        assert!(forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
    }

    /// T-2517, CC-76: inside a copy, what references the target is read on the copy's branch: a
    /// dependent the copy removed no longer blocks, and one the copy added does.
    #[tokio::test]
    async fn a_workspace_branch_delete_reads_dependents_on_that_branch_not_main() {
        const REPO: &str = "/api/v1/repos/test-owner/test-repo";
        let air = "projects/ovzdusie/endpoints/air.yaml";
        let new = "projects/ovzdusie/pipelines/new-sync.yaml";
        let space = "projects/ovzdusie/spaces/mobility/space.yaml";
        let forge = MockServer::start().await;
        let tree = |entries: Vec<(&str, &str)>| {
            let tree: Vec<Value> = entries
                .iter()
                .map(|(p, sha)| json!({ "path": p, "type": "blob", "sha": sha }))
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({ "tree": tree, "truncated": false }))
        };
        let file = |content: String| {
            use base64::Engine;
            ResponseTemplate::new(200).set_body_json(json!({
                "sha": "x", "encoding": "base64",
                "content": base64::engine::general_purpose::STANDARD.encode(content),
            }))
        };
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/branches/workspace/mobility-v2")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": "ws1" } })),
            )
            .mount(&forge)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/git/trees/base1")))
            .respond_with(tree(vec![(space, "s1"), (air, "a1")]))
            .mount(&forge)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/git/trees/ws1")))
            .respond_with(tree(vec![(space, "s1"), (new, "n1")]))
            .mount(&forge)
            .await;
        let yaml = |kind: &str, name: &str| {
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: {kind}\nmetadata:\n  name: {name}\n  namespace: ovzdusie\nspec:\n  contextSpaceRef:\n    kind: ContextSpace\n    name: mobility\n")
        };
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/contents/{air}")))
            .and(query_param("ref", "base1"))
            .respond_with(file(yaml("Endpoint", "air")))
            .mount(&forge)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/contents/{new}")))
            .and(query_param("ref", "workspace/mobility-v2"))
            .respond_with(file(yaml("Pipeline", "new-sync")))
            .mount(&forge)
            .await;
        let state = state_with(
            Some(&forge),
            vec![manifest("Endpoint", "air", HERE, space_ref(None))],
        );
        state
            .workspaces
            .create(Opening {
                name: "mobility-v2",
                title: None,
                project: HERE,
                owner: "jana@hel.fi",
                base_revision: "base1",
                scope: Scope::Project {},
                ttl_hours: 2,
            })
            .await
            .expect("opened");

        // On main, the Endpoint blocks it.
        let (main, _) = referenced(
            remove(&state, &steward(), "spaces", "mobility", true, None)
                .await
                .expect("main"),
        );
        assert_eq!(main, refs(&[("Endpoint", "air")]));
        // In the copy, the Endpoint is gone and the Pipeline the copy added blocks it instead.
        let (copy, elsewhere) = referenced(
            remove(
                &state,
                &steward(),
                "spaces",
                "mobility",
                true,
                Some("mobility-v2"),
            )
            .await
            .expect("copy"),
        );
        assert_eq!((copy, elsewhere), (refs(&[("Pipeline", "new-sync")]), 0));
    }
}
