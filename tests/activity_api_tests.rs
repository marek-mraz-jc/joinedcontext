//! The activity routes: what a project's members read, what the live tail sends, and what the
//! collector's ingest accepts (UI-31, OPS-48, OPS-49).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use chrono::Utc;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::activity::ActivityEvent;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const PROJECT: &str = "helsinki";
const CSRF: &str = "test-csrf-activity";

fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "ContextSpace".into(),
        metadata: ObjectMeta {
            name: "air-quality".into(),
            namespace: Some(PROJECT.into()),
            ..Default::default()
        },
        spec: json!({}),
        status: None,
    });
    mirror
}

fn cookie(config: &Config, username: &str) -> String {
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("sub-{username}"),
            username: username.to_owned(),
            email: None,
            name: Some(username.to_owned()),
            roles: vec!["portal-approver".to_owned()],
            groups: vec![],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &session).expect("a session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

fn event(kind: &str, severity: &str, object: &str) -> ActivityEvent {
    ActivityEvent {
        time: Utc::now(),
        project: PROJECT.into(),
        space: Some("air-quality".into()),
        kind: kind.into(),
        source: "gateway".into(),
        summary: "An anonymous caller was refused a write.".into(),
        severity: severity.into(),
        correlation_id: None,
        details: json!({ "object": object }),
    }
}

async fn call(
    app: &axum::Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header(CSRF_HEADER, CSRF);
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(body).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_member_reads_the_project_s_activity_and_filters_it() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    state
        .activity
        .append(&[
            event("access.denied", "warning", "endpoints/public-air"),
            event("pipeline.error", "error", "pipelines/aq-ingest"),
            event("endpoint.traffic", "info", "endpoints/public-air"),
        ])
        .await
        .expect("appended");
    let app = server::app(state);
    let jana = cookie(&config, "jana.kovacova");

    let (status, list) = call(
        &app,
        &jana,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/activity"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"].as_array().map(Vec::len), Some(3));
    assert_eq!(list["kind"], json!("List"));

    // A severity names the floor, an object narrows to itself, and a kind to itself.
    for (query, expected) in [
        ("?severity=warning", 2),
        ("?object=pipelines/aq-ingest", 1),
        ("?kind=access.denied,endpoint.traffic", 2),
        ("?space=air-quality", 3),
        ("?source=broker", 0),
    ] {
        let (status, list) = call(
            &app,
            &jana,
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/activity{query}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{query}: {list}");
        assert_eq!(
            list["items"].as_array().map(Vec::len),
            Some(expected),
            "{query}"
        );
    }

    // A parameter that is not one of this route's is refused rather than ignored.
    let (status, _) = call(
        &app,
        &jana,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/activity?severity=loud"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_project_that_does_not_exist_is_not_found_rather_than_forbidden() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    let app = server::app(state);
    let jana = cookie(&config, "jana.kovacova");

    let (status, _) = call(
        &app,
        &jana,
        Method::GET,
        "/api/v1/projects/Not_A_Project/activity",
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an activity route that tells the two apart says which projects exist (R20)"
    );
}

#[tokio::test]
async fn the_ingest_route_is_the_collector_s_and_not_a_person_s() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    let app = server::app(state);
    let jana = cookie(&config, "jana.kovacova");

    let (status, _) = call(
        &app,
        &jana,
        Method::POST,
        "/api/v1/activity",
        Some(json!({ "resourceLogs": [] })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a browser session is not the collector"
    );
}

#[tokio::test]
async fn a_page_is_the_size_it_asks_for_and_its_cursor_fetches_the_rest() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    let mut events = Vec::new();
    for _ in 0..5 {
        events.push(event("access.denied", "warning", "endpoints/public-air"));
    }
    state.activity.append(&events).await.expect("appended");
    let app = server::app(state);
    let jana = cookie(&config, "jana.kovacova");

    let (status, first) = call(
        &app,
        &jana,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/activity?limit=2"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["items"].as_array().map(Vec::len), Some(2));
    let cursor = first["next"].as_str().expect("a next page").to_owned();

    let (status, second) = call(
        &app,
        &jana,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/activity?limit=2&cursor={cursor}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["items"].as_array().map(Vec::len), Some(2));

    let (status, _) = call(
        &app,
        &jana,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/activity?cursor=not-a-cursor"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// The DB half (OPS-49): runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test
/// may write to, and says so and returns otherwise, like the other database suites.
async fn activity_db() -> Option<(sqlx::PgPool, String)> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty());
    let Some(url) = url else {
        eprintln!(
            "JC_PORTAL_TEST_DATABASE_URL unset: the activity trim against PostgreSQL is skipped"
        );
        return None;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    Some((pool, format!("trim-{nanos}")))
}

async fn rows_of(pool: &sqlx::PgPool, project: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM activity WHERE project = $1")
        .bind(project)
        .fetch_one(pool)
        .await
        .expect("a count")
}

#[tokio::test]
async fn one_trim_tick_drops_what_is_past_retention_and_the_next_within_the_hour_drops_nothing() {
    use joinedcontext_portal::activity::{ActivityStore, RETENTION_DAYS, TRIM_EVERY_MINUTES};
    let Some((pool, project)) = activity_db().await else {
        return;
    };
    let now = Utc::now();
    let store = ActivityStore::new(Some(pool.clone()));
    let at = |age: chrono::Duration| {
        let mut e = event("config.applied", "info", "endpoints/public-air");
        e.project = project.clone();
        e.time = now - age;
        e
    };
    let old = chrono::Duration::days(RETENTION_DAYS + 1);
    let new = chrono::Duration::days(1);
    store
        .append(&[at(old), at(old), at(old), at(new), at(new)])
        .await
        .expect("appended");

    let first = store.trim_if_due(now).await.expect("due").expect("trimmed");
    assert!(first.rows >= 3, "{first:?}");
    assert_eq!(rows_of(&pool, &project).await, 2);

    store.append(&[at(old)]).await.expect("appended");
    let soon = now + chrono::Duration::minutes(TRIM_EVERY_MINUTES - 1);
    assert!(store.trim_if_due(soon).await.is_none());
    assert_eq!(rows_of(&pool, &project).await, 3);
}

#[tokio::test]
async fn a_backlog_is_trimmed_in_bounded_batches() {
    use joinedcontext_portal::activity::{ActivityStore, RETENTION_DAYS, TRIM_BATCH};
    let Some((pool, project)) = activity_db().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO activity (time, project, kind, source, summary, severity, details) \
         SELECT now() - make_interval(days => $2::int), $1, 'config.applied', 'reconciler', \
                'Old.', 'info', '{}'::jsonb FROM generate_series(1, 25000)",
    )
    .bind(&project)
    .bind(i32::try_from(RETENTION_DAYS + 1).expect("days fit"))
    .execute(&pool)
    .await
    .expect("seeded");

    let done = ActivityStore::new(Some(pool.clone()))
        .trim(Utc::now())
        .await
        .expect("trimmed");
    assert_eq!(rows_of(&pool, &project).await, 0);
    assert!(done.rows >= 25_000, "{done:?}");
    // 25,000 of this test's rows alone take 3; other suites' old rows add to both counts alike.
    assert_eq!(
        u64::from(done.batches),
        done.rows.div_ceil(TRIM_BATCH as u64)
    );
    assert!(done.batches >= 3, "{done:?}");
}

#[tokio::test]
async fn a_postgres_page_keeps_one_connectors_events_and_pages_on_by_cursor() {
    use chrono::SubsecRound;
    use joinedcontext_portal::activity::{ActivityFilter, ActivityStore, Cursor};
    let Some((pool, project)) = activity_db().await else {
        return;
    };
    // The store keeps whole seconds (`chrono_to_odt`); the id orders events of one second.
    let now = Utc::now().trunc_subsecs(0);
    let store = ActivityStore::new(Some(pool));
    let call = |minutes: i64, connector: &str| {
        let mut e = event("mcp.tool", "info", "endpoints/public-air");
        e.project = project.clone();
        e.time = now - chrono::Duration::minutes(minutes);
        e.details = json!({ "connector": connector });
        e
    };
    let ours = "abcdefghijklmnopqrstuvwxyz";
    store
        .append(&[
            call(3, ours),
            call(2, "bbcdefghijklmnopqrstuvwxyz"),
            call(1, ours),
        ])
        .await
        .expect("appended");
    let mut filter = ActivityFilter {
        connector: Some(ours.into()),
        limit: 1,
        ..Default::default()
    };

    let first = store.list(&project, &filter).await.expect("a page");
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].time, call(1, ours).time);
    let next = first.next.expect("a second page");
    filter.cursor = Some(Cursor::decode(&next).expect("our own cursor"));
    let second = store.list(&project, &filter).await.expect("a page");
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].details["connector"], ours);
    assert_eq!(second.items[0].time, call(3, ours).time);
    assert_eq!(second.next, None);
}
