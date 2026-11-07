use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::api::dry_run::DryRunResult;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::Lane;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ProblemDetails;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::MockServer;

const TEST_CSRF_TOKEN: &str = "test-csrf-dry-run-token";

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
async fn post_dry_run_all_returns_200_and_makes_zero_git_requests() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "isSandbox": true
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces?dryRun=All")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let res: DryRunResult = serde_json::from_slice(&body_bytes).expect("deserialize DryRunResult");
    assert!(res.valid);
    assert_eq!(res.lane, Lane::Green);
    assert_eq!(res.plan.summary.create, 1);
    assert_eq!(res.plan.summary.update, 0);
    assert_eq!(res.plan.summary.delete, 0);

    let requests = server.received_requests().await.expect("received requests");
    assert!(requests.is_empty());
}

#[tokio::test]
async fn put_dry_run_all_returns_200_and_makes_zero_git_requests() {
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

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "isSandbox": true
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility?dryRun=All")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let res: DryRunResult = serde_json::from_slice(&body_bytes).expect("deserialize DryRunResult");
    assert!(res.valid);
    assert_eq!(res.lane, Lane::Green);
    assert_eq!(res.plan.summary.create, 0);
    assert_eq!(res.plan.summary.update, 1);
    assert_eq!(res.plan.summary.delete, 0);

    let requests = server.received_requests().await.expect("received requests");
    assert!(requests.is_empty());
}

#[tokio::test]
async fn delete_dry_run_all_returns_200_and_makes_zero_git_requests() {
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
                .uri("/api/v1/projects/ovzdusie/spaces/mobility?dryRun=All")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let res: DryRunResult = serde_json::from_slice(&body_bytes).expect("deserialize DryRunResult");
    assert!(res.valid);
    assert_eq!(res.lane, Lane::Red);
    assert_eq!(res.plan.summary.create, 0);
    assert_eq!(res.plan.summary.update, 0);
    assert_eq!(res.plan.summary.delete, 1);

    let requests = server.received_requests().await.expect("received requests");
    assert!(requests.is_empty());
}

#[tokio::test]
async fn invalid_dry_run_query_value_returns_400() {
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

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "isSandbox": true
        }
    });

    // 1. POST with ?dryRun=Foo
    let resp_post = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces?dryRun=Foo")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_post.status(), StatusCode::BAD_REQUEST);
    let bytes_post = resp_post
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_post: ProblemDetails = serde_json::from_slice(&bytes_post).expect("ProblemDetails");
    assert_eq!(problem_post.status, 400);
    assert_eq!(
        problem_post.r#type,
        "https://joinedcontext.com/errors/invalid-request"
    );
    assert!(problem_post
        .detail
        .as_deref()
        .expect("detail")
        .contains("invalid dryRun value 'Foo'; 'All' is the only accepted value"));

    // 2. PUT with ?dryRun=Foo
    let resp_put = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility?dryRun=Foo")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_put.status(), StatusCode::BAD_REQUEST);
    let bytes_put = resp_put
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_put: ProblemDetails = serde_json::from_slice(&bytes_put).expect("ProblemDetails");
    assert_eq!(problem_put.status, 400);
    assert!(problem_put
        .detail
        .as_deref()
        .expect("detail")
        .contains("invalid dryRun value 'Foo'; 'All' is the only accepted value"));

    // 3. DELETE with ?dryRun=Foo
    let resp_del = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility?dryRun=Foo")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_del.status(), StatusCode::BAD_REQUEST);
    let bytes_del = resp_del
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_del: ProblemDetails = serde_json::from_slice(&bytes_del).expect("ProblemDetails");
    assert_eq!(problem_del.status, 400);
    assert!(problem_del
        .detail
        .as_deref()
        .expect("detail")
        .contains("invalid dryRun value 'Foo'; 'All' is the only accepted value"));
}

/// One request's status and JSON body, with the session given.
async fn answer(
    app: &axum::Router,
    cookie: &str,
    method: &str,
    uri: &str,
    body: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// What a dry run decided, without the parts minted per call (the verdict's id and time).
fn decided(body: &serde_json::Value) -> serde_json::Value {
    json!({
        "valid": body["valid"],
        "lane": body["lane"],
        "plan": body["plan"],
        "findings": body["findings"],
        "type": body["type"],
        "errors": body["errors"],
    })
}

fn manifest(kind: &str, name: &str, spec: serde_json::Value) -> serde_json::Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": kind,
        "metadata": { "name": name, "namespace": "ovzdusie" },
        "spec": spec,
    })
}

/// `validate_only=true` (AIP-163) is `dryRun=All` on every route family of the resource API
/// (T-5949): the same status and the same verdict, and nothing reaches Git.
#[tokio::test]
async fn validate_only_answers_what_dry_run_answers_and_writes_nothing() {
    let server = MockServer::start().await;
    let client = GiteaClient::new(
        server.uri().parse().expect("url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
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
    let cookie = session_and_csrf_cookies(&config);

    let space = manifest("ContextSpace", "mobility", json!({ "isSandbox": true }));
    let cases = [
        (
            "POST",
            "/api/v1/projects/ovzdusie/spaces",
            manifest("ContextSpace", "transit", json!({ "isSandbox": true })),
        ),
        (
            "PUT",
            "/api/v1/projects/ovzdusie/spaces/mobility",
            space.clone(),
        ),
        (
            "DELETE",
            "/api/v1/projects/ovzdusie/spaces/mobility",
            json!({}),
        ),
        (
            "POST",
            "/api/v1/projects/ovzdusie/pipelines",
            manifest("Pipeline", "aq-ingest", json!({ "space": "mobility" })),
        ),
        (
            "POST",
            "/api/v1/projects/ovzdusie/datamodels",
            manifest("DataModel", "air", json!({ "space": "mobility" })),
        ),
        (
            "POST",
            "/api/v1/projects/ovzdusie/endpoints",
            manifest(
                "Endpoint",
                "air-quality-public-readers",
                json!({ "space": "mobility" }),
            ),
        ),
        (
            "POST",
            "/api/v1/projects/ovzdusie/policies",
            manifest("Policy", "readers", json!({ "rules": [] })),
        ),
        (
            "POST",
            "/api/v1/projects/ovzdusie/rolebindings",
            manifest(
                "RoleBinding",
                "readers",
                json!({ "role": "viewer", "subjects": [] }),
            ),
        ),
    ];
    for (method, path, body) in cases {
        let (dry, dry_body) =
            answer(&app, &cookie, method, &format!("{path}?dryRun=All"), &body).await;
        let (only, only_body) = answer(
            &app,
            &cookie,
            method,
            &format!("{path}?validate_only=true"),
            &body,
        )
        .await;
        assert_eq!(only, dry, "{method} {path}: {only_body}");
        assert_eq!(decided(&only_body), decided(&dry_body), "{method} {path}");
        if only.is_client_error() {
            assert!(
                only_body["type"].as_str().is_some(),
                "{method} {path} refuses with a problem document: {only_body}"
            );
        }
    }
    assert_eq!(
        decided(
            &answer(
                &app,
                &cookie,
                "POST",
                "/api/v1/projects/ovzdusie/spaces?validate_only=true",
                &manifest("ContextSpace", "transit", json!({ "isSandbox": true }))
            )
            .await
            .1
        )["valid"],
        json!(true),
        "a valid manifest validates"
    );
    let requests = server.received_requests().await.expect("recorded");
    assert!(
        requests.iter().all(|r| r.method.as_str() == "GET"),
        "validate_only wrote to Git: {:?}",
        requests
            .iter()
            .map(|r| format!("{} {}", r.method, r.url))
            .collect::<Vec<_>>()
    );
}

/// A caller without a role in the project learns no more by validating than by writing: the
/// refusal comes before the checks, the same on both doors (T-5949, KNOWLEDGE T-5248).
#[tokio::test]
async fn validate_only_refuses_a_stranger_before_the_checks() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let app = server::app(state);
    let stranger = {
        use axum::response::IntoResponse;
        let now = session::now_unix();
        let s = Session {
            identity: Identity {
                client: None,
                subject: "f:1:someone".into(),
                username: "someone".into(),
                email: None,
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: false,
            },
            expires_at: now + 3600,
            issued_at: now,
            signed_in_at: None,
            id_token: "id-token-placeholder".into(),
            access_expires_at: now + 3600,
            refresh_token: None,
        };
        let jar =
            session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("session");
        let response = (jar, StatusCode::OK).into_response();
        let session_cookie = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok()?.split(';').next().map(str::to_owned))
            .collect::<Vec<_>>()
            .join("; ");
        format!("{session_cookie}; {CSRF_COOKIE}={TEST_CSRF_TOKEN}")
    };
    let body = manifest("ContextSpace", "transit", json!({ "isSandbox": true }));
    let path = "/api/v1/projects/ovzdusie/spaces";
    let (written, _) = answer(&app, &stranger, "POST", path, &body).await;
    let (only, only_body) = answer(
        &app,
        &stranger,
        "POST",
        &format!("{path}?validate_only=true"),
        &body,
    )
    .await;
    assert!(
        only.is_client_error() && only != StatusCode::OK,
        "{only}: {only_body}"
    );
    assert_eq!(only, written, "{only_body}");
    assert!(
        only_body["plan"].is_null(),
        "no plan for a stranger: {only_body}"
    );
}
