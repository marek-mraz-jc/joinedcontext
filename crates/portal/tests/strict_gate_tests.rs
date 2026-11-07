//! Tests for strict validation gating and Verdict-based change proposal enforcement (AG-62, UI-48, PF-57).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::MockServer;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::ops::verdict::{Finding, Level, Verdict};
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const TEST_CSRF_TOKEN: &str = "test-csrf-token-strict-456";

fn session_cookie(
    config: &Config,
    username: &str,
    email: Option<&str>,
    roles: Vec<&str>,
    groups: Vec<&str>,
) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: format!("sub-{username}"),
            username: username.to_string(),
            email: email.map(str::to_string),
            name: Some(username.to_string()),
            roles: roles.into_iter().map(str::to_string).collect(),
            groups: groups.into_iter().map(str::to_string).collect(),
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
    parts.push(format!("{CSRF_COOKIE}={TEST_CSRF_TOKEN}"));
    parts.join("; ")
}

async fn setup_mock_gitea() -> (MockServer, GiteaClient) {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = GiteaClient::new(base_url, "owner", "repo", "token").unwrap();

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/v1/repos/owner/repo"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(
            "/api/v1/repos/owner/repo/branches",
        ))
        .respond_with(wiremock::ResponseTemplate::new(201).set_body_json(json!({
            "name": "portal/create-datasource-feed-strict-12345678"
        })))
        .mount(&server)
        .await;

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(wiremock::ResponseTemplate::new(404))
        .mount(&server)
        .await;

    wiremock::Mock::given(wiremock::matchers::method("PUT"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(wiremock::ResponseTemplate::new(201).set_body_json(json!({
            "commit": { "sha": "commit-sha-created" }
        })))
        .mount(&server)
        .await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v1/repos/owner/repo/pulls"))
        .respond_with(wiremock::ResponseTemplate::new(201).set_body_json(json!({
            "number": 42,
            "html_url": "https://git.example/pulls/42",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    (server, client)
}

#[tokio::test]
async fn strict_mode_propose_without_verdict_refused_409() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-strict", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-strict",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state);
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let payload = json!({
        "draft": {
            "kind": "DataSource",
            "name": "feed-strict"
        }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_resource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "verdict_required");
    assert_eq!(body["check"], "jc_datasource_check");
}

#[tokio::test]
async fn strict_mode_check_runs_and_attaches_verdict_then_propose_succeeds() {
    let (_gitea_server, gitea_client) = setup_mock_gitea().await;

    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None)
        .with_mirror(mirror)
        .with_gitea(Arc::new(gitea_client));

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-strict", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-strict",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state.clone());
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let check_payload = json!({
        "draft": {
            "kind": "DataSource",
            "name": "feed-strict"
        }
    });

    let resp_check = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_datasource_check")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&check_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_check.status(), StatusCode::OK);
    let check_bytes = resp_check.into_body().collect().await.unwrap().to_bytes();
    let check_body: Value = serde_json::from_slice(&check_bytes).unwrap();
    assert_eq!(check_body["verdict"]["ok"], true);

    let draft_after_check = state
        .drafts
        .get("ovzdusie", "DataSource", "feed-strict")
        .await
        .unwrap()
        .expect("draft exists");
    assert!(draft_after_check.verdict.is_some());
    assert!(draft_after_check.verdict.as_ref().unwrap().ok);

    let propose_payload = json!({
        "draft": {
            "kind": "DataSource",
            "name": "feed-strict"
        }
    });

    let resp_propose = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_resource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&propose_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_propose.status(), StatusCode::ACCEPTED);
    let prop_bytes = resp_propose.into_body().collect().await.unwrap().to_bytes();
    let prop_body: Value = serde_json::from_slice(&prop_bytes).unwrap();
    assert!(prop_body.get("changeId").is_some() || prop_body.get("change").is_some());

    assert!(state
        .drafts
        .get("ovzdusie", "DataSource", "feed-strict")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn strict_mode_stale_verdict_refused_409() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-stale", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-stale",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state.clone());
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let check_payload = json!({
        "draft": { "kind": "DataSource", "name": "feed-stale" }
    });
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_datasource_check")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&check_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let updated_manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-stale", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/updated_bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-stale",
            updated_manifest,
            Some(1),
            "steward",
            "person",
        )
        .await
        .unwrap();

    let propose_payload = json!({
        "draft": { "kind": "DataSource", "name": "feed-stale" }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_resource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&propose_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "verdict_required");
    assert_eq!(body["reason"], "stale");
    assert_eq!(
        body["detail"],
        "The draft changed since its check; check it again, then propose it."
    );
}

#[tokio::test]
async fn strict_mode_red_verdict_refused_409() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-red", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-red",
            manifest.clone(),
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let red_verdict = Verdict::red(
        &manifest,
        vec![Finding {
            level: Level::Error,
            path: "/spec/http/url".into(),
            message: "feed unreachable".into(),
        }],
        None,
    );
    state
        .drafts
        .set_verdict("ovzdusie", "DataSource", "feed-red", red_verdict)
        .await
        .unwrap();

    let app = server::app(state);
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let propose_payload = json!({
        "draft": { "kind": "DataSource", "name": "feed-red" }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_resource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&propose_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "verdict_required");
    assert_eq!(body["reason"], "verdict_failed");
}

#[tokio::test]
async fn lax_mode_allows_propose_with_warning() {
    let (_gitea_server, gitea_client) = setup_mock_gitea().await;

    let branding_dir = std::env::temp_dir();
    let branding_path = branding_dir.join(format!("jc-branding-lax-{}.yaml", std::process::id()));
    std::fs::write(&branding_path, "validation: lax\n").expect("write branding file");

    let mut config = Config::for_tests();
    config.branding_file = Some(branding_path.to_string_lossy().to_string());

    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None)
        .with_mirror(mirror)
        .with_gitea(Arc::new(gitea_client));

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-lax", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-lax",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state.clone());
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let propose_payload = json!({
        "draft": { "kind": "DataSource", "name": "feed-lax" }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_resource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&propose_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let _ = std::fs::remove_file(&branding_path);

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["warning"], "proposed without a fresh green verdict");
    assert!(state
        .drafts
        .get("ovzdusie", "DataSource", "feed-lax")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn pipeline_propose_requires_jc_pipeline_test() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "Pipeline",
        "metadata": { "name": "pipe-strict", "namespace": "ovzdusie" },
        "spec": {
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "source-1" } },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:org:ovzdusie:all"
        }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "Pipeline",
            "pipe-strict",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state);
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let payload = json!({
        "draft": { "kind": "Pipeline", "name": "pipe-strict" }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_resource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "verdict_required");
    assert_eq!(body["check"], "jc_pipeline_test");
}

#[tokio::test]
async fn space_propose_requires_jc_manifest_dry_run() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "space-strict", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "ContextSpace",
            "space-strict",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state);
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let payload = json!({
        "draft": { "kind": "ContextSpace", "name": "space-strict" }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_resource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "verdict_required");
    assert_eq!(body["check"], "jc_manifest_dry_run");
}

/// One request through the whole router as the steward; the status and the JSON answer.
async fn rest(app: &axum::Router, method: &str, uri: &str, body: &Value) -> (StatusCode, Value) {
    let config_cookie = STEWARD.with(|cookie| cookie.borrow().clone());
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::COOKIE, config_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

thread_local! {
    static STEWARD: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// A strict Portal (the default) against the mock forge, the steward signed in; the forge server
/// is returned so a test can see that nothing reached it.
async fn strict_portal() -> (MockServer, AppState, axum::Router) {
    let (gitea_server, gitea_client) = setup_mock_gitea().await;
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None)
        .with_mirror(Arc::new(Mirror::new()))
        .with_gitea(Arc::new(gitea_client));
    assert_eq!(
        state.branding().validation,
        joinedcontext_portal::branding::Validation::Strict
    );
    let cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );
    STEWARD.with(|steward| *steward.borrow_mut() = cookie);
    let app = server::app(state.clone());
    (gitea_server, state, app)
}

fn space(name: &str, sandbox: bool) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": name, "namespace": "ovzdusie" },
        "spec": { "isSandbox": sandbox }
    })
}

async fn branches_created(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/branches"))
        .count()
}

/// Owner decision T-0956 (PF-57): every door is gated. A bare manifest posted to the REST route,
/// never checked, is refused with the gate's own document and nothing reaches the forge; the
/// same manifest checked on the same route first is proposed, and the draft its check created is
/// gone afterwards.
#[tokio::test]
async fn a_rest_proposal_naming_no_draft_needs_its_own_check() {
    let (forge, state, app) = strict_portal().await;
    let uri = "/api/v1/projects/ovzdusie/spaces";
    let manifest = space("air", true);

    let (status, refused) = rest(&app, "POST", uri, &manifest).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "verdict_required");
    assert_eq!(refused["check"], "jc_manifest_dry_run");
    assert_eq!(refused["reason"], "verdict_absent");
    assert_eq!(
        branches_created(&forge).await,
        0,
        "nothing reached the forge"
    );

    let (status, checked) = rest(&app, "POST", &format!("{uri}?dryRun=All"), &manifest).await;
    assert_eq!(status, StatusCode::OK, "{checked}");
    assert_eq!(checked["verdict"]["ok"], true, "{checked}");

    let (status, change) = rest(&app, "POST", uri, &manifest).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{change}");
    assert!(
        state
            .drafts
            .get("ovzdusie", "ContextSpace", "air")
            .await
            .unwrap()
            .is_none(),
        "the draft the check created is forgotten once proposed"
    );
}

/// A check is fresh for the manifest it judged and no other: the same resource proposed with other
/// content is refused as stale, and a manifest the check refused leaves nothing to propose on.
#[tokio::test]
async fn a_changed_or_refused_manifest_is_not_proposed_on_an_earlier_check() {
    let (forge, _state, app) = strict_portal().await;
    let uri = "/api/v1/projects/ovzdusie/spaces";

    let (status, _) = rest(
        &app,
        "POST",
        &format!("{uri}?dryRun=All"),
        &space("air", true),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, stale) = rest(&app, "POST", uri, &space("air", false)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    assert_eq!(stale["reason"], "stale");

    let mut invalid = space("water", true);
    invalid["spec"]["isSandbox"] = json!("not a flag");
    let (status, refused) = rest(&app, "POST", &format!("{uri}?dryRun=All"), &invalid).await;
    // The check refuses the manifest itself, as the red verdict that says why (T-2234), so the
    // gate below has no green verdict to find.
    assert_eq!(status, StatusCode::OK, "{refused}");
    assert_eq!(refused["verdict"]["ok"], false, "{refused}");
    let (status, body) = rest(&app, "POST", uri, &invalid).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a manifest that fails its own checks says so before the gate: {body}"
    );
    assert_eq!(branches_created(&forge).await, 0);
}

/// The operations registry is a door like the REST route: a bare ContextSpace manifest proposed
/// through `jc_resource_propose` needs the verdict `jc_manifest_dry_run` records for it, and a
/// person's own draft of the same resource, with other content, survives a bare proposal of it.
#[tokio::test]
async fn the_operation_door_gates_a_bare_manifest_and_leaves_a_persons_draft_alone() {
    let (_forge, state, app) = strict_portal().await;
    let ops = "/api/v1/projects/ovzdusie/ops";
    let manifest = space("air", true);

    let (status, refused) = rest(
        &app,
        "POST",
        &format!("{ops}/jc_resource_propose"),
        &json!({ "manifest": manifest }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "verdict_required");

    let (status, checked) = rest(
        &app,
        "POST",
        &format!("{ops}/jc_manifest_dry_run"),
        &json!({ "manifest": manifest }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{checked}");
    // Somebody opens the same space in a form and changes it after the check. The verdict is
    // still fresh for the manifest the operation proposes, which is the one that was checked.
    let theirs = space("air", false);
    state
        .drafts
        .put(
            "ovzdusie",
            "ContextSpace",
            "air",
            theirs.clone(),
            None,
            "someone.else",
            "person",
        )
        .await
        .unwrap();
    let (status, change) = rest(
        &app,
        "POST",
        &format!("{ops}/jc_resource_propose"),
        &json!({ "manifest": manifest }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{change}");
    assert_eq!(change["changeId"], "chg-0000002a", "{change}");
    let kept = state
        .drafts
        .get("ovzdusie", "ContextSpace", "air")
        .await
        .unwrap()
        .expect("their draft survives a bare proposal of the same space");
    assert_eq!(kept.manifest, theirs);
}

/// The refusal names the check of the kind: a DataSource is checked by `jc_datasource_check`.
#[tokio::test]
async fn an_unchecked_data_source_is_told_to_run_the_data_source_check() {
    let (_forge, _state, app) = strict_portal().await;
    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-unchecked", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });
    let (status, refused) = rest(
        &app,
        "POST",
        "/api/v1/projects/ovzdusie/datasources",
        &manifest,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["check"], "jc_datasource_check");
}

/// A lax installation lets an unchecked bare manifest through, as it lets an unchecked draft.
#[tokio::test]
async fn a_lax_portal_proposes_an_unchecked_bare_manifest() {
    let (_gitea_server, gitea_client) = setup_mock_gitea().await;
    let branding_path =
        std::env::temp_dir().join(format!("jc-branding-lax-bare-{}.yaml", std::process::id()));
    std::fs::write(&branding_path, "validation: lax\n").expect("write branding file");
    let mut config = Config::for_tests();
    config.branding_file = Some(branding_path.to_string_lossy().to_string());
    let state = AppState::new(config.clone(), None)
        .with_mirror(Arc::new(Mirror::new()))
        .with_gitea(Arc::new(gitea_client));
    STEWARD.with(|steward| {
        *steward.borrow_mut() = session_cookie(
            &config,
            "steward.user",
            Some("steward@banskabystrica.sk"),
            vec!["portal-approver"],
            vec![],
        )
    });
    let app = server::app(state);
    let (status, body) = rest(
        &app,
        "POST",
        "/api/v1/projects/ovzdusie/spaces",
        &space("air", true),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

#[tokio::test]
async fn rest_door_with_a_draft_reaches_the_check_and_the_gate() {
    let (_gitea_server, gitea_client) = setup_mock_gitea().await;
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None)
        .with_mirror(mirror)
        .with_gitea(Arc::new(gitea_client));

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-rest", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });
    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-rest",
            manifest.clone(),
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state.clone());
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );
    let mut with_draft = manifest.clone();
    with_draft["draft"] = json!({ "kind": "DataSource", "name": "feed-rest" });
    let post = |uri: &str| {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::COOKIE, &steward_cookie)
            .header(CSRF_HEADER, TEST_CSRF_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&with_draft).unwrap()))
            .unwrap()
    };

    // Strict, no verdict yet: the gate answers through the generic door too.
    let resp = app
        .clone()
        .oneshot(post("/api/v1/projects/ovzdusie/datasources"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["error"], "verdict_required");
    assert_eq!(body["check"], "jc_datasource_check");
    // A page shows the refusal as it is: what happened and what to do (T-0769).
    assert_eq!(
        body["detail"],
        "The draft has not been checked; check it, then propose it."
    );

    // The dry run is the check: it answers a verdict and files it on the draft.
    let resp = app
        .clone()
        .oneshot(post("/api/v1/projects/ovzdusie/datasources?dryRun=All"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["verdict"]["ok"], true);
    let draft = state
        .drafts
        .get("ovzdusie", "DataSource", "feed-rest")
        .await
        .unwrap()
        .expect("draft exists");
    assert!(draft.verdict.as_ref().is_some_and(|v| v.ok));

    // With a fresh green verdict the proposal is a change, and the draft is gone.
    let resp = app
        .oneshot(post("/api/v1/projects/ovzdusie/datasources"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    // The draft door answers the Change envelope itself, as the plain door does: the page's
    // notice reads metadata.name and status.lane from it.
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["kind"], "Change", "{body}");
    assert!(body["metadata"]["name"].is_string(), "{body}");
    assert!(body["status"]["lane"].is_string(), "{body}");
    assert!(state
        .drafts
        .get("ovzdusie", "DataSource", "feed-rest")
        .await
        .unwrap()
        .is_none());

    // AG-77, T-0842: a kind without a propose operation of its own goes the same way. The form
    // of a Dashboard sends its draft to the collection route and gets a Change, not a 400.
    // The endpoint its widget reads is in the project, because a dashboard drawing one that is not
    // there is refused by the check (MF-13, T-2268).
    state.mirror.upsert(
        serde_json::from_value(json!({
            "apiVersion": API_VERSION,
            "kind": "Endpoint",
            "metadata": { "name": "bikes", "namespace": "ovzdusie" },
            "spec": {
                "contextSpaceRef": "ovzdusie",
                "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y",
                "audience": "project-list",
                "enabledRepresentations": ["ngsi-ld"]
            }
        }))
        .expect("an envelope"),
    );
    let dashboard = json!({
        "apiVersion": API_VERSION,
        "kind": "Dashboard",
        "metadata": { "name": "bikes-board", "namespace": "ovzdusie" },
        "spec": { "title": "Bikes", "pages": [{ "title": "Map", "layout": "grid-2x2",
                     "widgets": [{ "widgetType": "value", "endpointRef": "bikes" }] }] }
    });
    state
        .drafts
        .put(
            "ovzdusie",
            "Dashboard",
            "bikes-board",
            dashboard.clone(),
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();
    let mut with_draft = dashboard.clone();
    with_draft["draft"] = json!({ "kind": "Dashboard", "name": "bikes-board" });
    let post = |uri: &str| {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::COOKIE, &steward_cookie)
            .header(CSRF_HEADER, TEST_CSRF_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&with_draft).unwrap()))
            .unwrap()
    };
    let app = server::app(state.clone());
    // The same gate holds for it: unchecked is a conflict, never a silent proposal.
    let resp = app
        .clone()
        .oneshot(post("/api/v1/projects/ovzdusie/dashboards"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let resp = app
        .clone()
        .oneshot(post("/api/v1/projects/ovzdusie/dashboards?dryRun=All"))
        .await
        .unwrap();
    let status = resp.status();
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let resp = app
        .oneshot(post("/api/v1/projects/ovzdusie/dashboards"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["kind"], "Change", "{body}");
}

#[tokio::test]
async fn a_check_naming_an_unsaved_draft_creates_it_so_propose_follows_at_once() {
    let (_gitea_server, gitea_client) = setup_mock_gitea().await;
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None)
        .with_mirror(Arc::new(Mirror::new()))
        .with_gitea(Arc::new(gitea_client));
    let app = server::app(state.clone());
    let cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );
    let call = |op: &str, payload: Value| {
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/projects/ovzdusie/ops/{op}"))
            .header(header::COOKIE, &cookie)
            .header(CSRF_HEADER, TEST_CSRF_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&payload).unwrap()))
            .unwrap()
    };
    let draft = json!({ "kind": "DataSource", "name": "feed-strict" });
    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-strict", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });

    // The form's debounced save has not run: no draft yet when Check is pressed.
    let check = app
        .clone()
        .oneshot(call(
            "jc_datasource_check",
            json!({ "draft": draft, "manifest": manifest }),
        ))
        .await
        .unwrap();
    assert_eq!(check.status(), StatusCode::OK);

    let saved = state
        .drafts
        .get("ovzdusie", "DataSource", "feed-strict")
        .await
        .unwrap()
        .expect("the check created the draft");
    assert_eq!(saved.manifest, manifest);
    assert_eq!(saved.touched_by, "steward.user");
    assert!(saved.verdict.as_ref().is_some_and(|v| v.ok));

    let propose = app
        .oneshot(call("jc_resource_propose", json!({ "draft": draft })))
        .await
        .unwrap();
    assert_eq!(propose.status(), StatusCode::ACCEPTED);
}
