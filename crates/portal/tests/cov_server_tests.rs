//! The Portal's surface and its listeners (server.rs): the metrics path, the frame sources, the
//! internal and build listeners, the background loops and a start that cannot bind or build.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use joinedcontext_portal::apps::build_pods::Settings;
use joinedcontext_portal::config::{AgentSettings, Config};
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::reconciler::views::ViewSync;
use joinedcontext_portal::reconciler::Syncer;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use wiremock::MockServer;

fn dead_pool() -> sqlx::PgPool {
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .expect("a lazy pool")
}

fn build_settings() -> Settings {
    Settings {
        namespace: "builds".into(),
        api: "http://portal.example.org:9092".into(),
        bind: "127.0.0.1:0".parse().expect("an address"),
        node_image: "node@sha256:1".into(),
        rust_image: "rust@sha256:2".into(),
        cache_size: "1Gi".into(),
        tmp_size: "4Gi".into(),
        work_dir: std::env::temp_dir(),
    }
}

fn verifying(mut state: AppState) -> AppState {
    let issuer: url::Url = "https://id.example.org/realms/r".parse().expect("a url");
    state.bearer = Some(Arc::new(
        joinedcontext_portal::auth::bearer::BearerVerifier::new(&issuer, "portal"),
    ));
    state
}

async fn status_of(app: axum::Router, uri: &str) -> (StatusCode, axum::http::HeaderMap) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    (response.status(), response.headers().clone())
}

#[tokio::test]
async fn the_metrics_path_answers_not_found_when_it_is_not_public() {
    let mut config = Config::for_tests();
    config.public_metrics = false;
    let (status, _) = status_of(server::app(AppState::new(config, None)), "/metrics").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_metrics_path_is_scraped_when_it_is_public() {
    let mut config = Config::for_tests();
    config.public_metrics = true;
    let (status, _) = status_of(server::app(AppState::new(config, None)), "/metrics").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn an_apps_url_without_a_host_frames_only_itself() {
    let mut config = Config::for_tests();
    config.apps_url = Some("data:text/plain,apps".parse().expect("a url"));
    let (_, headers) = status_of(server::app(AppState::new(config, None)), "/catalogue").await;
    let policy = headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .expect("ASCII");
    assert!(policy.contains("frame-src 'self';"), "{policy}");
}

#[tokio::test]
async fn an_apps_url_with_a_host_frames_its_apps_under_it() {
    let mut config = Config::for_tests();
    config.apps_url = Some("https://apps.example.org/".parse().expect("a url"));
    let (_, headers) = status_of(server::app(AppState::new(config, None)), "/catalogue").await;
    let policy = headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .expect("ASCII");
    assert!(
        policy.contains("https://*.apps.apps.example.org"),
        "{policy}"
    );
}

#[tokio::test]
async fn the_internal_router_and_the_build_router_refuse_an_unknown_path() {
    let state = AppState::new(Config::for_tests(), None);
    let (internal, _) = status_of(server::internal_app(state.clone()), "/nowhere").await;
    assert_eq!(internal, StatusCode::NOT_FOUND);
    let (build, _) = status_of(server::build_app(state), "/nowhere").await;
    assert_eq!(build, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_portal_without_a_realm_opens_no_internal_listener() {
    let state = AppState::new(Config::for_tests(), None);
    let opened = server::spawn_internal_listener(&state, std::future::ready(())).await;
    assert_eq!(opened.expect("no error"), None);
}

#[tokio::test]
async fn a_portal_with_a_realm_serves_its_internal_listener_until_shutdown() {
    let state = verifying(AppState::new(Config::for_tests(), None));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let addr = server::spawn_internal_listener(&state, async {
        let _ = stopped.await;
    })
    .await
    .expect("bound")
    .expect("a listener");
    let answer = reqwest::get(format!("http://{addr}/nowhere"))
        .await
        .expect("an answer");
    assert_eq!(answer.status(), reqwest::StatusCode::NOT_FOUND);
    stop.send(()).expect("the listener still waits");
}

#[tokio::test]
async fn an_internal_listener_bound_by_the_caller_answers_on_its_address() {
    let state = AppState::new(Config::for_tests(), None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let expected = listener.local_addr().expect("address");
    let addr =
        server::serve_internal_listener(&state, listener, std::future::pending()).expect("serving");
    assert_eq!(addr, expected);
    let answer = reqwest::get(format!("http://{addr}/nowhere"))
        .await
        .expect("answer");
    assert_eq!(answer.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_internal_address_already_in_use_is_an_error() {
    let taken = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let mut config = Config::for_tests();
    config.internal_bind = taken.local_addr().expect("address");
    let state = verifying(AppState::new(config, None));
    let opened = server::spawn_internal_listener(&state, std::future::ready(())).await;
    assert_eq!(
        opened.expect_err("in use").kind(),
        std::io::ErrorKind::AddrInUse
    );
}

#[tokio::test]
async fn a_portal_that_builds_no_apps_opens_no_build_listener() {
    let state = AppState::new(Config::for_tests(), None);
    let opened = server::spawn_build_listener(&state, std::future::ready(())).await;
    assert_eq!(opened.expect("no error"), None);
}

#[tokio::test]
async fn a_build_namespace_without_a_database_opens_no_build_listener() {
    let mut config = Config::for_tests();
    config.build_pods = Some(build_settings());
    let state = AppState::new(config, None);
    assert!(state.db.is_none());
    let opened = server::spawn_build_listener(&state, std::future::ready(())).await;
    assert_eq!(opened.expect("no error"), None);
}

#[tokio::test]
async fn a_portal_that_builds_apps_serves_its_build_listener_until_shutdown() {
    let mut config = Config::for_tests();
    config.build_pods = Some(build_settings());
    let mut state = AppState::new(config, None);
    state.db = Some(dead_pool());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let addr = server::spawn_build_listener(&state, async {
        let _ = stopped.await;
    })
    .await
    .expect("bound")
    .expect("a listener");
    let answer = reqwest::get(format!("http://{addr}/nowhere"))
        .await
        .expect("answer");
    assert_eq!(answer.status(), reqwest::StatusCode::NOT_FOUND);
    stop.send(()).expect("the listener still waits");
}

#[tokio::test]
async fn a_build_address_already_in_use_is_an_error() {
    let taken = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let mut settings = build_settings();
    settings.bind = taken.local_addr().expect("address");
    let mut config = Config::for_tests();
    config.build_pods = Some(settings);
    let mut state = AppState::new(config, None);
    state.db = Some(dead_pool());
    let opened = server::spawn_build_listener(&state, std::future::ready(())).await;
    assert_eq!(
        opened.expect_err("in use").kind(),
        std::io::ErrorKind::AddrInUse
    );
}

/// A state with a forge, a syncer, a sync driver, a database, a build namespace and an agent
/// runner: every loop `spawn_background` can start.
fn busy_state(forge: &MockServer, with_views: bool) -> AppState {
    let mut config = Config::for_tests();
    config.build_pods = Some(build_settings());
    config.agent_settings = Some(AgentSettings {
        namespace: Some("agents".into()),
        portal_namespace: "portal".into(),
        proxy_base: forge.uri(),
        run_ttl_secs: 600,
        approval_ttl_secs: 600,
    });
    let mut state = AppState::new(config, None);
    let shako = Arc::new(
        ShakoClient::new(
            forge.uri().parse().expect("forge url"),
            "bb",
            "org",
            "token",
        )
        .expect("shako client"),
    );
    let mut syncer = Syncer::new(Arc::clone(&shako), Arc::clone(&state.mirror));
    if with_views {
        syncer = syncer.with_views(Arc::new(ViewSync::new(
            "http://127.0.0.1:1",
            "/nonexistent",
        )));
    }
    state.syncer = Some(Arc::new(syncer));
    state.sync = Some(Arc::new(joinedcontext_portal::sync::driver::Driver::new(
        Arc::clone(&shako),
        Arc::new(joinedcontext_portal::sync::state::States::new(None)),
        Arc::new(joinedcontext_portal::sync::remote::HttpRemote::new().expect("a client")),
    )));
    state.shako = Some(shako);
    state.db = Some(dead_pool());
    state
}

#[tokio::test]
async fn the_background_work_starts_with_a_broker_writing_views() {
    let forge = MockServer::start().await;
    server::spawn_background(&busy_state(&forge, true));
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn the_background_work_starts_with_a_portal_that_writes_no_views() {
    let forge = MockServer::start().await;
    server::spawn_background(&busy_state(&forge, false));
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// A forge and a syncer without an outbound client for `SyncSource`s: the sync loop and the
/// mirror start, the `SyncSource` loop does not.
#[tokio::test]
async fn the_background_work_starts_without_a_sync_driver() {
    let forge = MockServer::start().await;
    let mut state = busy_state(&forge, false);
    state.sync = None;
    server::spawn_background(&state);
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn the_background_work_of_a_bare_portal_starts_without_a_database() {
    server::spawn_background(&AppState::new(Config::for_tests(), None));
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn a_start_on_a_taken_address_is_an_error() {
    let taken = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let mut config = Config::for_tests();
    config.bind = taken.local_addr().expect("address");
    let err = server::serve(config).await.expect_err("in use");
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
}

#[tokio::test]
async fn a_start_against_a_database_that_is_down_is_an_error() {
    let mut config = Config::for_tests();
    config.database_url = Some("postgres://nobody@127.0.0.1:1/none".into());
    let err = server::serve(config).await.expect_err("no database");
    assert_eq!(err.kind(), std::io::ErrorKind::Other);
    assert!(!err.to_string().is_empty());
}
