//! The metrics route in front of a Bento runner: what a session may read, and what happens when
//! the runner is absent (PL-24, PL-17).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, Phase, ResourceEnvelope, Status, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// One runner for every project (T-3002): another project's pipeline of the same name, and the
/// stream a Portal before T-3002 kept under the bare name, are not this pipeline's counters.
const RUNNER_BODY: &str = concat!(
    "# TYPE input_received counter\n",
    "input_received{label=\"mqtt\",stream=\"ovzdusie.aq-mqtt-ingest\"} 128401\n",
    "output_sent{label=\"gw\",stream=\"ovzdusie.aq-mqtt-ingest\"} 128390\n",
    "output_error{label=\"gw\",stream=\"ovzdusie.aq-mqtt-ingest\"} 2\n",
    "input_received{label=\"mqtt\",stream=\"praha.aq-mqtt-ingest\"} 5000\n",
    "input_received{label=\"mqtt\",stream=\"aq-mqtt-ingest\"} 700\n",
    "input_received{label=\"mqtt\",stream=\"ovzdusie.parking-feed\"} 9\n",
);

fn session_cookie(config: &Config) -> String {
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
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

fn mirror_with_pipeline() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Pipeline".into(),
        metadata: ObjectMeta {
            name: "aq-mqtt-ingest".into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        },
        spec: serde_json::json!({ "class": "resident" }),
        status: Some(Status {
            phase: Phase::Live,
            observed_revision: None,
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        }),
    });
    mirror
}

async fn get(config: Config, uri: &str) -> (StatusCode, serde_json::Value) {
    let cookie = session_cookie(&config);
    let app = server::app(AppState::new(config, None).with_mirror(mirror_with_pipeline()));
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn anonymous_metrics_call_returns_401() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config, None).with_mirror(mirror_with_pipeline()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/pipelines/aq-mqtt-ingest/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn scrapes_the_runner_of_the_project_in_the_path() {
    let runner = MockServer::start().await;
    // Mounted under the project's own path: a runner URL that ignored `{project}` would 404 here.
    Mock::given(method("GET"))
        .and(path("/ovzdusie/metrics"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RUNNER_BODY))
        .mount(&runner)
        .await;

    let mut config = Config::for_tests();
    // `{project}` in the template is what makes one setting serve every project's runner.
    config.pipeline_runner_url = Some(format!("{}/{{project}}", runner.uri()));

    let (status, body) = get(
        config,
        "/api/v1/projects/ovzdusie/pipelines/aq-mqtt-ingest/metrics",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pipeline"], "aq-mqtt-ingest");
    assert_eq!(body["received"], 128401);
    assert_eq!(body["sent"], 128390);
    assert_eq!(body["errors"], 2);
    assert!(
        body["scrapedAt"].as_str().is_some_and(|s| s.contains('T')),
        "the answer says when it was read"
    );
    assert!(
        body.get("bufferDepth").is_none(),
        "a counter the runner does not export stays absent, it is not reported as zero"
    );
}

#[tokio::test]
async fn without_a_configured_runner_the_route_answers_503() {
    let (status, body) = get(
        Config::for_tests(),
        "/api/v1/projects/ovzdusie/pipelines/aq-mqtt-ingest/metrics",
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], 503);
}

#[tokio::test]
async fn a_runner_that_refuses_is_unavailable_not_an_internal_error() {
    let runner = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/metrics"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&runner)
        .await;

    let mut config = Config::for_tests();
    config.pipeline_runner_url = Some(runner.uri());

    let (status, body) = get(
        config,
        "/api/v1/projects/ovzdusie/pipelines/aq-mqtt-ingest/metrics",
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains(&runner.uri()),
        "the internal runner URL is never handed to the caller"
    );
}

#[tokio::test]
async fn a_pipeline_that_is_not_mirrored_is_404_before_any_scrape() {
    let runner = MockServer::start().await;
    // No mock is mounted: reaching the runner at all would fail this test.
    let mut config = Config::for_tests();
    config.pipeline_runner_url = Some(runner.uri());

    let (status, _) = get(
        config,
        "/api/v1/projects/ovzdusie/pipelines/not-there/metrics",
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_project_name_that_is_not_a_dns_label_never_reaches_the_url_builder() {
    let mut config = Config::for_tests();
    config.pipeline_runner_url = Some("http://runner.invalid/{project}".into());

    let (status, _) = get(
        config,
        "/api/v1/projects/OVZDUSIE/pipelines/aq-mqtt-ingest/metrics",
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// T-0983, PF-59: the counters say a pipeline of this name runs here and how it is doing, so a
/// caller with no grant in the project is answered as if it were not there. Otherwise anyone
/// with a login maps another department's pipelines by name.
#[tokio::test]
async fn a_caller_with_no_grant_is_not_told_the_pipeline_runs() {
    use axum::response::IntoResponse;
    // A runner that would answer, so the refusal is the permission check and not a missing
    // runner: without the check this call returned the counters.
    let runner = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ovzdusie/metrics"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RUNNER_BODY))
        .mount(&runner)
        .await;
    let mut config = Config::for_tests();
    config.pipeline_runner_url = Some(format!("{}/{{project}}", runner.uri()));
    let now = session::now_unix();
    let outsider = Session {
        identity: Identity {
            client: None,
            subject: "f:1:passer.by".into(),
            username: "passer.by".into(),
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
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &outsider).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let cookie = response
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or_default().to_owned())
        .expect("a session cookie");

    let app = server::app(AppState::new(config, None).with_mirror(mirror_with_pipeline()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/pipelines/aq-mqtt-ingest/metrics")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "a project the caller may not read answers 404, never its counters"
    );
}

/// PL-68: the stats route answers per node and window from the same scrape.
#[tokio::test]
async fn stats_count_per_node_from_the_runner() {
    let runner = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ovzdusie/metrics"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RUNNER_BODY))
        .mount(&runner)
        .await;
    let mut config = Config::for_tests();
    config.pipeline_runner_url = Some(format!("{}/{{project}}", runner.uri()));

    let (status, body) = get(
        config,
        "/api/v1/projects/ovzdusie/pipelines/aq-mqtt-ingest/stats",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pipeline"], "aq-mqtt-ingest");
    assert_eq!(body["coveredSeconds"], 0);
    assert_eq!(
        body["nodes"]["gw"]["1m"]["errors"], 0,
        "one reading is no rate yet"
    );
    assert_eq!(body["nodes"]["mqtt"]["15m"]["received"], 0);
    assert!(
        body["nodes"]["mqtt"]["1m"].get("errors").is_none(),
        "a counter the runner does not export stays absent"
    );
    assert_eq!(
        body["nodes"].as_object().map(|n| n.len()),
        Some(2),
        "another project's stream of the same name is not ours"
    );
}

#[tokio::test]
async fn stats_of_a_runner_that_is_down_are_503_never_zeros() {
    let runner = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&runner)
        .await;
    let mut config = Config::for_tests();
    config.pipeline_runner_url = Some(runner.uri());

    let (status, body) = get(
        config,
        "/api/v1/projects/ovzdusie/pipelines/aq-mqtt-ingest/stats",
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.get("nodes").is_none());
}

#[tokio::test]
async fn stats_of_an_unknown_pipeline_are_404_before_any_scrape() {
    let runner = MockServer::start().await;
    let mut config = Config::for_tests();
    config.pipeline_runner_url = Some(runner.uri());

    let (status, _) = get(
        config,
        "/api/v1/projects/ovzdusie/pipelines/not-there/stats",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(
        Config::for_tests(),
        "/api/v1/projects/praha/pipelines/aq-mqtt-ingest/stats",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another project's pipeline is not there"
    );
}
