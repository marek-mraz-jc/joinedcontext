//! The preferences tier against a real PostgreSQL (UI-09, UI-10): migrations, the round trip,
//! subject isolation and the fail-closed answer without a database.
//!
//! The database tests run when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may
//! write to (locally: `docker run -e POSTGRES_PASSWORD=… postgres:17-alpine`); otherwise they
//! skip with a note, so the fast lane without a service container stays green.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

const CSRF: &str = "csrf-token-value";

fn session_cookie(config: &Config, subject: &str) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: subject.into(),
            username: "demo.steward".into(),
            email: None,
            name: None,
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
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect();
    parts.push(format!("__Host-jc_csrf={CSRF}"));
    parts.join("; ")
}

async fn call(
    app: &axum::Router,
    cookie: &str,
    method: Method,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri("/api/v1/preferences")
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", CSRF);
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn database_url() -> Option<String> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL").ok()?;
    if url.trim().is_empty() {
        return None;
    }
    Some(url)
}

/// A subject nobody else's test writes to, so tests share one database without a fixture.
fn fresh_subject(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("test:{tag}:{nanos}")
}

#[tokio::test]
async fn without_a_database_the_routes_answer_503() {
    let config = Config::for_tests();
    let cookie = session_cookie(&config, "f:1:demo.steward");
    let app = server::app(AppState::new(config, None));

    let (status, body) = call(&app, &cookie, Method::GET, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], 503);

    let (status, _) = call(&app, &cookie, Method::PUT, Some(json!({ "theme": "dark" }))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

/// T-6520: the walker met a 500 here while dev's database was short of connections. A pool that
/// hands out no connection is a dependency down: 503 with a next step, on every door of this file.
#[tokio::test]
async fn a_database_that_does_not_answer_is_503_with_a_next_step() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address")
        .port();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(200))
        .connect_lazy(&format!("postgres://portal@127.0.0.1:{closed}/portal"))
        .expect("a lazy pool");
    let config = Config::for_tests();
    let cookie = session_cookie(&config, "f:1:demo.steward");
    let mut state = AppState::new(config, None);
    state.db = Some(pool);
    let app = server::app(state);

    let (status, body) = call(&app, &cookie, Method::GET, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["status"], 503);
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("try again"), "{detail}");
    assert!(!detail.contains("127.0.0.1"), "{detail}");

    let (status, _) = call(&app, &cookie, Method::PUT, Some(json!({ "theme": "dark" }))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn anonymous_calls_are_401_before_the_database_is_asked() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/preferences")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn migrates_saves_and_reads_back_immediately() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    // Connecting runs the embedded migrations; a second connect finds nothing to do.
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    joinedcontext_portal::db::connect(&url)
        .await
        .expect("migrations are idempotent");

    let config = Config::for_tests();
    let subject = fresh_subject("roundtrip");
    let cookie = session_cookie(&config, &subject);
    let app = server::app(AppState::new(config, None).with_db(pool));

    let (status, body) = call(&app, &cookie, Method::GET, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({}),
        "nothing saved yet is an empty object, not a 404"
    );

    let wanted = json!({
        "theme": "dark",
        "locale": "sk",
        "defaultProject": "ovzdusie",
        "dashboardLayouts": { "ovzdusie-prehlad": { "collapsedLegend": true } }
    });
    let started = std::time::Instant::now();
    let (status, body) = call(&app, &cookie, Method::PUT, Some(wanted.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, wanted);

    let (status, body) = call(&app, &cookie, Method::GET, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the write is visible on the very next read (UI-10)"
    );
    assert_eq!(body, wanted);
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "a write and a read took {elapsed:?}; the tier is a primary-key lookup, not a pipeline"
    );

    // A PUT replaces the document whole: a field left out is cleared.
    let (status, body) = call(
        &app,
        &cookie,
        Method::PUT,
        Some(json!({ "theme": "light" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "theme": "light" }));
    let (_, body) = call(&app, &cookie, Method::GET, None).await;
    assert_eq!(body, json!({ "theme": "light" }));
}

#[tokio::test]
async fn one_subject_never_sees_another_subjects_row() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let config = Config::for_tests();
    let alice = session_cookie(&config, &fresh_subject("alice"));
    let bob = session_cookie(&config, &fresh_subject("bob"));
    let app = server::app(AppState::new(config, None).with_db(pool));

    let (status, _) = call(&app, &alice, Method::PUT, Some(json!({ "locale": "de" }))).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(&app, &bob, Method::GET, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({}),
        "Bob's row is his own, empty until he saves"
    );
}

#[tokio::test]
async fn an_invalid_field_is_400_and_leaves_the_row_untouched() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let config = Config::for_tests();
    let cookie = session_cookie(&config, &fresh_subject("invalid"));
    let app = server::app(AppState::new(config, None).with_db(pool));

    let (status, _) = call(&app, &cookie, Method::PUT, Some(json!({ "theme": "dark" }))).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(
        &app,
        &cookie,
        Method::PUT,
        Some(json!({ "theme": "sepia" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["status"], 400);
    let (status, body) = call(&app, &cookie, Method::PUT, Some(json!({ "colour": "red" }))).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unknown fields are refused, not stored"
    );
    assert_eq!(body["status"], 400);

    let (_, body) = call(&app, &cookie, Method::GET, None).await;
    assert_eq!(body, json!({ "theme": "dark" }));
}

/// UI-90: the recent pages a tab reports are put first, each once, the newest ten kept, and
/// nothing else of the stored preferences changes.
#[tokio::test]
async fn recent_pages_are_merged_without_touching_the_rest() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let config = Config::for_tests();
    let cookie = session_cookie(&config, &fresh_subject("recent"));
    let app = server::app(AppState::new(config, None).with_db(pool));
    let place = |path: &str| json!({ "path": path, "title": "t" });
    let (status, _) = call(
        &app,
        &cookie,
        Method::PUT,
        Some(json!({ "theme": "dark", "recent": [place("/a"), place("/b")] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let post = |body: Value| {
        let app = app.clone();
        let cookie = cookie.clone();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/preferences/recent")
                        .header(header::COOKIE, cookie)
                        .header("x-csrf-token", CSRF)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };
    let (status, body) = post(json!({ "places": [place("/c"), place("/b")] })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["theme"], "dark");
    assert_eq!(
        body["recent"],
        json!([place("/c"), place("/b"), place("/a")])
    );

    let (status, _) = post(json!({ "places": [] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(json!({ "places": [place("https://evil.example/")] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, stored) = call(&app, &cookie, Method::GET, None).await;
    assert_eq!(
        stored["recent"],
        json!([place("/c"), place("/b"), place("/a")]),
        "a refused report changes nothing"
    );
}
